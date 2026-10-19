//! Typed wrappers over [`GraphClient::get`] for the endpoints used today
//! (SPEC §11): teams, channels, channel files, delta, transfers, messages.

use std::path::Path;

use reqwest::header::CONTENT_RANGE;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::{GRAPH_BASE, GraphClient, json_body, merge_page};
use crate::error::{Error, Result};

/// Graph's cut-off for a single `PUT …:/content` upload.
pub const SMALL_UPLOAD_LIMIT: usize = 4 * 1024 * 1024;

/// Upload-session chunks must be a multiple of 320 KiB; 5 MiB is one.
const CHUNK_SIZE: usize = 5 * 1024 * 1024;

#[derive(Debug, Deserialize)]
pub struct Team {
    pub id: String,
    #[serde(rename = "displayName", default)]
    pub display_name: String,
}

#[derive(Debug, Deserialize)]
pub struct Channel {
    pub id: String,
    #[serde(rename = "displayName", default)]
    pub display_name: String,
    #[serde(rename = "webUrl", default)]
    pub web_url: Option<String>,
}

/// A file or folder in a SharePoint drive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DriveItem {
    pub id: String,
    pub name: String,
    pub size: Option<u64>,
    /// RFC 3339, as Graph sends it.
    pub last_modified: Option<String>,
    pub web_url: Option<String>,
    pub is_folder: bool,
    pub parent_drive_id: Option<String>,
    pub parent_id: Option<String>,
    /// Set on delta entries for removed items.
    pub deleted: bool,
}

/// One full delta walk: every changed item plus the link for the next walk.
#[derive(Debug, Default)]
pub struct Delta {
    pub items: Vec<DriveItem>,
    pub delta_link: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    pub id: String,
    /// `from.user.displayName`, empty for system and bot messages.
    pub author: String,
    /// `createdDateTime`, RFC 3339.
    pub created: String,
    /// `body.content`; Teams always sends HTML when asked with `$expand`.
    pub body_html: String,
    pub replies: Vec<Message>,
}

impl GraphClient {
    pub async fn joined_teams(&self) -> Result<Vec<Team>> {
        let values = self.get_all("/me/joinedTeams").await?;
        Ok(serde_json::from_value(Value::Array(values))?)
    }

    pub async fn channels(&self, team_id: &str) -> Result<Vec<Channel>> {
        let values = self.get_all(&format!("/teams/{team_id}/channels")).await?;
        Ok(serde_json::from_value(Value::Array(values))?)
    }

    /// The channel's folder in the team's SharePoint drive. Its `id` is the
    /// folder and `parent_drive_id` the drive the other calls need.
    pub async fn files_folder(&self, team_id: &str, channel_id: &str) -> Result<DriveItem> {
        let path = format!("/teams/{team_id}/channels/{channel_id}/filesFolder");
        let value = self.get(&path).await?;
        drive_item_from(&value).ok_or_else(|| not_an_item("filesFolder"))
    }

    pub async fn children(&self, drive_id: &str, item_id: &str) -> Result<Vec<DriveItem>> {
        let path = format!("/drives/{drive_id}/items/{item_id}/children");
        let values = self.get_all(&path).await?;
        Ok(values.iter().filter_map(drive_item_from).collect())
    }

    /// Changes under a folder since `delta_link`, or everything on the first
    /// call. The returned `delta_link` is what to pass next time; a
    /// [`Error::Graph`] with status 410 means it expired and the caller
    /// should start over with `None`.
    pub async fn delta(
        &self,
        drive_id: &str,
        item_id: &str,
        delta_link: Option<&str>,
    ) -> Result<Delta> {
        let first = match delta_link {
            Some(link) => link.to_string(),
            None => format!("/drives/{drive_id}/items/{item_id}/delta"),
        };
        let mut delta = Delta::default();
        let mut next = Some(first);
        while let Some(url) = next {
            let page = self.get(&url).await?;
            next = merge_delta_page(&mut delta, page);
        }
        Ok(delta)
    }

    /// Starts a download; the caller streams the body.
    ///
    /// Graph answers with a 302 to a pre-authenticated SharePoint url. reqwest
    /// follows it and, because the host changes, drops the `Authorization`
    /// header on the way (`redirect::remove_sensitive_headers` in reqwest
    /// 0.13), so the Bearer token never reaches SharePoint.
    pub async fn download(&self, drive_id: &str, item_id: &str) -> Result<reqwest::Response> {
        let url = format!("{GRAPH_BASE}/drives/{drive_id}/items/{item_id}/content");
        let response = self.send(self.http.get(url)).await?;
        if !response.status().is_success() {
            return Err(crate::client::graph_error(response).await);
        }
        Ok(response)
    }

    /// Uploads a file of at most [`SMALL_UPLOAD_LIMIT`] bytes in one request,
    /// replacing an existing file of the same name.
    pub async fn upload_small(
        &self,
        drive_id: &str,
        parent_id: &str,
        name: &str,
        bytes: Vec<u8>,
    ) -> Result<DriveItem> {
        let url = named_item_url(drive_id, parent_id, name, "content");
        let response = self.send(self.http.put(url).body(bytes)).await?;
        let value = json_body(response).await?;
        drive_item_from(&value).ok_or_else(|| not_an_item("upload"))
    }

    /// Uploads a file of any size through an upload session in 5 MiB chunks.
    /// The session url is pre-authenticated, so the chunks carry no Bearer.
    pub async fn upload_large(
        &self,
        drive_id: &str,
        parent_id: &str,
        name: &str,
        path: &Path,
    ) -> Result<DriveItem> {
        let bytes = tokio::fs::read(path).await?;
        let session_url = named_item_url(drive_id, parent_id, name, "createUploadSession");
        let body = json!({"item": {"@microsoft.graph.conflictBehavior": "replace", "name": name}});
        let session = json_body(self.send(self.http.post(session_url).json(&body)).await?).await?;
        let Some(upload_url) = session.get("uploadUrl").and_then(Value::as_str) else {
            return Err(not_an_item("createUploadSession"));
        };

        let total = bytes.len();
        let mut last = Value::Null;
        for (offset, chunk) in (0..).step_by(CHUNK_SIZE).zip(bytes.chunks(CHUNK_SIZE)) {
            let response = self
                .http
                .put(upload_url)
                .header(CONTENT_RANGE, content_range(offset, chunk.len(), total))
                .body(chunk.to_vec())
                .send()
                .await?;
            last = json_body(response).await?;
        }
        // Intermediate chunks answer 202 with the next expected ranges; only
        // the final one carries the driveItem.
        drive_item_from(&last).ok_or_else(|| not_an_item("upload session"))
    }

    /// The newest `top` messages of a channel with their replies, newest first.
    pub async fn messages(
        &self,
        team_id: &str,
        channel_id: &str,
        top: u32,
    ) -> Result<Vec<Message>> {
        let path =
            format!("/teams/{team_id}/channels/{channel_id}/messages?$top={top}&$expand=replies");
        let page = self.get(&path).await?;
        let mut values = Vec::new();
        merge_page(&mut values, page);
        Ok(values.iter().filter_map(message_from).collect())
    }
}

pub fn drive_item_from(value: &Value) -> Option<DriveItem> {
    let id = text(value, "id")?;
    let parent = value.get("parentReference");
    Some(DriveItem {
        id,
        name: text(value, "name").unwrap_or_default(),
        size: value.get("size").and_then(Value::as_u64),
        last_modified: text(value, "lastModifiedDateTime"),
        web_url: text(value, "webUrl"),
        is_folder: value.get("folder").is_some(),
        parent_drive_id: parent.and_then(|p| text(p, "driveId")),
        parent_id: parent.and_then(|p| text(p, "id")),
        deleted: value.get("deleted").is_some(),
    })
}

pub fn message_from(value: &Value) -> Option<Message> {
    let id = text(value, "id")?;
    let author = value
        .get("from")
        .and_then(|from| from.get("user"))
        .and_then(|user| text(user, "displayName"))
        .unwrap_or_default();
    let body_html = value
        .get("body")
        .and_then(|body| text(body, "content"))
        .unwrap_or_default();
    let replies = value
        .get("replies")
        .and_then(Value::as_array)
        .map(|replies| replies.iter().filter_map(message_from).collect())
        .unwrap_or_default();
    Some(Message {
        id,
        author,
        created: text(value, "createdDateTime").unwrap_or_default(),
        body_html,
        replies,
    })
}

/// Folds one delta page into `delta` and returns the next page's url. The
/// `deltaLink` only appears on the last page, so it is kept whenever seen.
fn merge_delta_page(delta: &mut Delta, page: Value) -> Option<String> {
    if let Some(link) = text(&page, "@odata.deltaLink") {
        delta.delta_link = Some(link);
    }
    let mut values = Vec::new();
    let next = merge_page(&mut values, page);
    delta
        .items
        .extend(values.iter().filter_map(drive_item_from));
    next
}

/// `bytes 0-5242879/12000000`, as the upload session wants it.
fn content_range(offset: usize, len: usize, total: usize) -> String {
    format!("bytes {offset}-{}/{total}", offset + len - 1)
}

/// `…/items/{parent}:/{name}:/{action}`, with the file name percent-encoded.
fn named_item_url(drive_id: &str, parent_id: &str, name: &str, action: &str) -> String {
    let mut url = url::Url::parse(GRAPH_BASE).expect("constant url is valid");
    url.path_segments_mut()
        .expect("https url has segments")
        .extend([
            "drives",
            drive_id,
            "items",
            &format!("{parent_id}:"),
            &format!("{name}:"),
            action,
        ]);
    url.to_string()
}

fn text(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn not_an_item(what: &str) -> Error {
    Error::Graph {
        status: 200,
        code: "unexpectedResponse".to_string(),
        message: format!("{what} response is not a driveItem"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drive_item_from_file() {
        let value = json!({
            "id": "01ABC",
            "name": "lecture 1.pdf",
            "size": 123456,
            "lastModifiedDateTime": "2026-10-01T08:00:00Z",
            "webUrl": "https://contoso.sharepoint.com/sites/x/lecture%201.pdf",
            "file": {"mimeType": "application/pdf"},
            "parentReference": {"driveId": "b!drive", "id": "01PARENT", "path": "/drive/root:/General"}
        });
        let item = drive_item_from(&value).unwrap();
        assert_eq!(
            item,
            DriveItem {
                id: "01ABC".to_string(),
                name: "lecture 1.pdf".to_string(),
                size: Some(123456),
                last_modified: Some("2026-10-01T08:00:00Z".to_string()),
                web_url: Some("https://contoso.sharepoint.com/sites/x/lecture%201.pdf".to_string()),
                is_folder: false,
                parent_drive_id: Some("b!drive".to_string()),
                parent_id: Some("01PARENT".to_string()),
                deleted: false,
            }
        );
    }

    #[test]
    fn drive_item_from_folder() {
        let value = json!({
            "id": "01DIR",
            "name": "General",
            "folder": {"childCount": 4},
            "parentReference": {"driveId": "b!drive"}
        });
        let item = drive_item_from(&value).unwrap();
        assert!(item.is_folder);
        assert!(!item.deleted);
        assert_eq!(item.size, None);
        assert_eq!(item.parent_drive_id.as_deref(), Some("b!drive"));
        assert_eq!(item.parent_id, None);
    }

    #[test]
    fn drive_item_from_deleted_delta_entry() {
        let value = json!({
            "id": "01GONE",
            "name": "old.docx",
            "deleted": {"state": "deleted"},
            "parentReference": {"driveId": "b!drive", "id": "01PARENT"}
        });
        let item = drive_item_from(&value).unwrap();
        assert!(item.deleted);
        assert!(!item.is_folder);
    }

    #[test]
    fn drive_item_from_tolerates_missing_parent_and_rejects_missing_id() {
        let item = drive_item_from(&json!({"id": "root", "name": "root"})).unwrap();
        assert_eq!(item.parent_drive_id, None);
        assert_eq!(item.parent_id, None);
        assert!(drive_item_from(&json!({"name": "no id"})).is_none());
    }

    #[test]
    fn message_from_with_replies() {
        let value = json!({
            "id": "1700000000000",
            "createdDateTime": "2026-10-18T10:00:00Z",
            "from": {"user": {"displayName": "Ada Lovelace", "id": "u1"}},
            "body": {"contentType": "html", "content": "<p>Exam on Friday</p>"},
            "replies": [
                {
                    "id": "1700000001000",
                    "createdDateTime": "2026-10-18T10:05:00Z",
                    "from": {"user": {"displayName": "Bob"}},
                    "body": {"contentType": "text", "content": "Thanks"}
                },
                {
                    "id": "1700000002000",
                    "createdDateTime": "2026-10-18T10:06:00Z",
                    "from": {"application": {"displayName": "Bot"}},
                    "body": {"content": "<systemEventMessage/>"}
                }
            ]
        });
        let message = message_from(&value).unwrap();
        assert_eq!(message.author, "Ada Lovelace");
        assert_eq!(message.created, "2026-10-18T10:00:00Z");
        assert_eq!(message.body_html, "<p>Exam on Friday</p>");
        assert_eq!(message.replies.len(), 2);
        assert_eq!(message.replies[0].author, "Bob");
        assert_eq!(message.replies[0].body_html, "Thanks");
        assert_eq!(message.replies[1].author, "");
        assert!(message.replies[1].replies.is_empty());
    }

    #[test]
    fn message_from_without_from_or_replies() {
        let message = message_from(&json!({"id": "1", "body": {"content": "hi"}})).unwrap();
        assert_eq!(message.author, "");
        assert_eq!(message.created, "");
        assert!(message.replies.is_empty());
        assert!(message_from(&json!({"body": {"content": "no id"}})).is_none());
    }

    #[test]
    fn merge_delta_page_keeps_final_delta_link_across_pages() {
        let mut delta = Delta::default();
        let next = merge_delta_page(
            &mut delta,
            json!({
                "value": [{"id": "a", "name": "a.pdf"}, {"id": "d", "name": "d", "folder": {}}],
                "@odata.nextLink": "https://graph.microsoft.com/v1.0/drives/x/items/y/delta?token=1"
            }),
        );
        assert_eq!(
            next.as_deref(),
            Some("https://graph.microsoft.com/v1.0/drives/x/items/y/delta?token=1")
        );
        assert!(delta.delta_link.is_none());

        let next = merge_delta_page(
            &mut delta,
            json!({
                "value": [{"id": "b", "name": "b.pdf", "deleted": {}}],
                "@odata.deltaLink": "https://graph.microsoft.com/v1.0/drives/x/items/y/delta?token=final"
            }),
        );
        assert!(next.is_none());
        assert_eq!(
            delta.delta_link.as_deref(),
            Some("https://graph.microsoft.com/v1.0/drives/x/items/y/delta?token=final")
        );
        let ids: Vec<&str> = delta.items.iter().map(|i| i.id.as_str()).collect();
        assert_eq!(ids, ["a", "d", "b"]);
        assert!(delta.items[1].is_folder);
        assert!(delta.items[2].deleted);
    }

    #[test]
    fn content_range_formats_inclusive_ranges() {
        assert_eq!(
            content_range(0, 5 * 1024 * 1024, 12_000_000),
            "bytes 0-5242879/12000000"
        );
        assert_eq!(
            content_range(10_485_760, 1_514_240, 12_000_000),
            "bytes 10485760-11999999/12000000"
        );
        assert_eq!(content_range(0, 1, 1), "bytes 0-0/1");
    }

    #[test]
    fn chunking_covers_the_whole_file() {
        let bytes = vec![0u8; 2 * CHUNK_SIZE + 7];
        let ranges: Vec<String> = (0..)
            .step_by(CHUNK_SIZE)
            .zip(bytes.chunks(CHUNK_SIZE))
            .map(|(offset, chunk)| content_range(offset, chunk.len(), bytes.len()))
            .collect();
        assert_eq!(
            ranges,
            [
                "bytes 0-5242879/10485767",
                "bytes 5242880-10485759/10485767",
                "bytes 10485760-10485766/10485767",
            ]
        );
    }

    #[test]
    fn named_item_url_encodes_the_file_name() {
        assert_eq!(
            named_item_url("b!drive", "01PARENT", "my file #1 50%.pdf", "content"),
            "https://graph.microsoft.com/v1.0/drives/b!drive/items/01PARENT:/my%20file%20%231%2050%25.pdf:/content"
        );
        assert_eq!(
            named_item_url("d", "p", "a/b.txt", "createUploadSession"),
            "https://graph.microsoft.com/v1.0/drives/d/items/p:/a%2Fb.txt:/createUploadSession"
        );
    }

    #[test]
    fn team_and_channel_deserialize_from_graph_shape() {
        let teams: Vec<Team> = serde_json::from_value(json!([
            {"id": "t1", "displayName": "Algorithms", "description": "x"},
            {"id": "t2"}
        ]))
        .unwrap();
        assert_eq!(teams[0].display_name, "Algorithms");
        assert_eq!(teams[1].display_name, "");

        let channels: Vec<Channel> = serde_json::from_value(json!([
            {"id": "c1", "displayName": "General", "webUrl": "https://teams.microsoft.com/l/channel/x"},
            {"id": "c2", "displayName": "Labs"}
        ]))
        .unwrap();
        assert_eq!(
            channels[0].web_url.as_deref(),
            Some("https://teams.microsoft.com/l/channel/x")
        );
        assert_eq!(channels[1].web_url, None);
    }
}
