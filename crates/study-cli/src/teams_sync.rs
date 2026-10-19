//! The Teams phase of `study sync` (SPEC §6): joined teams → channels →
//! each channel's files folder → a recursive crawl of its drive items.
//!
//! A team with a single channel is flattened: its files and messages hang
//! off the team node and the channel node is dropped (SPEC §4.2).

use std::time::Duration;

use anyhow::{Context, Result, bail};
use indicatif::ProgressBar;
use study_core::index::{File, Index, Node, NodeKind, Source, SourceKind};
use study_graph::GraphClient;
use study_graph::api::{Channel, DriveItem, Team};
use study_graph::client::GRAPH_BASE;

use crate::app;

/// A drive item becomes either a folder node or a file row.
#[derive(Debug, PartialEq)]
pub enum Mapped {
    Folder(Node),
    File(File),
}

/// Crawls every joined team into `index` and returns the summary fragment
/// for the sync line. A team that fails is skipped with a warning; only a
/// dead login or an unreachable Graph fails the phase.
pub async fn sync(graph: &GraphClient, index: &mut Index, quiet: bool) -> Result<String> {
    let teams = match graph.joined_teams().await {
        Ok(teams) => teams,
        Err(study_graph::Error::AuthExpired(_)) => {
            bail!("Teams login expired — run `study login teams`")
        }
        Err(err) => return Err(err).context("listing teams"),
    };

    let spinner = if quiet {
        ProgressBar::hidden()
    } else {
        ProgressBar::new_spinner()
    };
    spinner.enable_steady_tick(Duration::from_millis(100));
    let mut nodes = Vec::new();
    let mut files = Vec::new();
    let mut synced = 0;
    for team in &teams {
        spinner.set_message(team.display_name.clone());
        match crawl_team(graph, team, &mut nodes, &mut files).await {
            Ok(()) => synced += 1,
            Err(err) if !quiet => {
                eprintln!("warning: skipping team '{}': {err:#}", team.display_name);
            }
            Err(_) => {}
        }
    }
    spinner.finish_and_clear();

    let source = Source {
        kind: SourceKind::Teams,
        name: "Microsoft Teams".to_string(),
        remote_id: "teams".to_string(),
        last_sync: Some(app::now()),
        delta_token: None,
    };
    let summary = format!("{synced} teams, {} team files", files.len());
    index.replace_source(source, nodes, files, vec![]);
    app::persist_graph_token(graph)?;
    Ok(summary)
}

/// One team's nodes and files, appended to `nodes` and `files`.
async fn crawl_team(
    graph: &GraphClient,
    team: &Team,
    nodes: &mut Vec<Node>,
    files: &mut Vec<File>,
) -> Result<()> {
    let channels = graph.channels(&team.id).await.context("listing channels")?;
    let mut folders = Vec::new();
    for channel in channels {
        let folder = graph
            .files_folder(&team.id, &channel.id)
            .await
            .with_context(|| format!("locating the files of '{}'", channel.display_name))?;
        folders.push((channel, folder));
    }

    let flattened = folders.len() == 1;
    nodes.extend(map_team(team, &folders));
    for (channel, folder) in &folders {
        let parent = files_parent_id(team, channel, flattened);
        let Some(drive) = folder.parent_drive_id.as_deref() else {
            bail!("'{}' has no drive", channel.display_name);
        };
        crawl_folder(graph, drive, &folder.id, &parent, nodes, files)
            .await
            .with_context(|| format!("crawling '{}'", channel.display_name))?;
    }
    Ok(())
}

/// Depth-first over an explicit stack: an `async fn` cannot recurse without
/// boxing, and the order of index rows does not matter.
async fn crawl_folder(
    graph: &GraphClient,
    drive: &str,
    root: &str,
    parent: &str,
    nodes: &mut Vec<Node>,
    files: &mut Vec<File>,
) -> Result<()> {
    let mut pending = vec![(parent.to_string(), root.to_string())];
    while let Some((parent, item_id)) = pending.pop() {
        for item in graph.children(drive, &item_id).await? {
            match map_item(&parent, drive, &item) {
                Some(Mapped::Folder(node)) => {
                    pending.push((node.id.clone(), item.id.clone()));
                    nodes.push(node);
                }
                Some(Mapped::File(file)) => files.push(file),
                None => {}
            }
        }
    }
    Ok(())
}

/// The team node, plus a channel node per channel when there are several,
/// plus a messages module per channel. `channels` pairs each channel with
/// its files folder; its drive and id become the node's remote id.
pub fn map_team(team: &Team, channels: &[(Channel, DriveItem)]) -> Vec<Node> {
    let team_id = format!("teams:{}", team.id);
    let flattened = channels.len() == 1;
    let team_remote_id = match channels {
        [(_, folder)] => folder_remote_id(folder),
        _ => team.id.clone(),
    };
    let mut nodes = vec![Node {
        id: team_id.clone(),
        source: SourceKind::Teams,
        parent: None,
        kind: NodeKind::Team,
        modname: None,
        name: team.display_name.clone(),
        remote_id: team_remote_id,
        web_url: None,
    }];

    for (channel, folder) in channels {
        let parent = files_parent_id(team, channel, flattened);
        if !flattened {
            nodes.push(Node {
                id: parent.clone(),
                source: SourceKind::Teams,
                parent: Some(team_id.clone()),
                kind: NodeKind::Channel,
                modname: None,
                name: channel.display_name.clone(),
                remote_id: folder_remote_id(folder),
                web_url: channel.web_url.clone(),
            });
        }
        nodes.push(Node {
            id: format!("teams:msg:{}", channel.id),
            source: SourceKind::Teams,
            parent: Some(parent),
            kind: NodeKind::Module,
            modname: Some("messages".to_string()),
            name: format!("Messages ({})", channel.display_name),
            remote_id: format!("{}/{}", team.id, channel.id),
            web_url: channel.web_url.clone(),
        });
    }
    nodes
}

/// A folder becomes a node under `parent`, a file a row on it. Delta
/// entries for removed items carry no content and are dropped.
pub fn map_item(parent: &str, drive: &str, item: &DriveItem) -> Option<Mapped> {
    if item.deleted {
        return None;
    }
    let remote_id = format!("{drive}/{}", item.id);
    if item.is_folder {
        return Some(Mapped::Folder(Node {
            id: format!("teams:item:{}", item.id),
            source: SourceKind::Teams,
            parent: Some(parent.to_string()),
            kind: NodeKind::Folder,
            modname: None,
            name: item.name.clone(),
            remote_id,
            web_url: item.web_url.clone(),
        }));
    }
    Some(Mapped::File(File {
        id: format!("teams:file:{}", item.id),
        node: parent.to_string(),
        name: item.name.clone(),
        size: item.size,
        mtime: item.last_modified.as_deref().and_then(rfc3339_to_unix),
        mime: None,
        remote_id,
        download_url: format!("{GRAPH_BASE}/drives/{drive}/items/{}/content", item.id),
        web_url: item.web_url.clone(),
    }))
}

/// The node a channel's files and messages hang off: the team itself when
/// it is flattened, otherwise the channel node.
fn files_parent_id(team: &Team, channel: &Channel, flattened: bool) -> String {
    if flattened {
        return format!("teams:{}", team.id);
    }
    format!("teams:chan:{}", channel.id)
}

/// `<driveId>/<itemId>`; a files folder without a drive cannot be crawled,
/// so an empty drive here only ever reaches a node that gets no files.
fn folder_remote_id(folder: &DriveItem) -> String {
    let drive = folder.parent_drive_id.as_deref().unwrap_or_default();
    format!("{drive}/{}", folder.id)
}

/// Unix seconds from Graph's `YYYY-MM-DDTHH:MM:SS[.fff]Z`; anything else is
/// `None`. Just enough parsing to avoid a date crate for one field.
pub fn rfc3339_to_unix(text: &str) -> Option<u64> {
    let (date, time) = text.strip_suffix('Z')?.split_once('T')?;
    let time = time.split('.').next()?;
    let [year, month, day] = numbers::<3>(date, '-')?;
    let [hour, minute, second] = numbers::<3>(time, ':')?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let days = u64::try_from(days_from_civil(year, month, day)).ok()?;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second)
}

/// Exactly `N` unsigned numbers separated by `sep`.
fn numbers<const N: usize>(text: &str, sep: char) -> Option<[u64; N]> {
    let mut parts = text.split(sep).map(|part| part.parse::<u64>().ok());
    let mut out = [0; N];
    for slot in &mut out {
        *slot = parts.next()??;
    }
    if parts.next().is_some() {
        return None;
    }
    Some(out)
}

/// Days since 1970-01-01 in the proleptic Gregorian calendar (Howard
/// Hinnant's `days_from_civil`), negative before the epoch.
fn days_from_civil(year: u64, month: u64, day: u64) -> i64 {
    let (year, month, day) = (year as i64, month as i64, day as i64);
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let month_index = (month + 9) % 12;
    let day_of_year = (153 * month_index + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    fn team() -> Team {
        Team {
            id: "t1".to_string(),
            display_name: "Algorithms".to_string(),
        }
    }

    fn channel(id: &str, name: &str) -> (Channel, DriveItem) {
        let channel = Channel {
            id: id.to_string(),
            display_name: name.to_string(),
            web_url: Some(format!("https://teams.microsoft.com/l/channel/{id}")),
        };
        let folder = DriveItem {
            id: format!("root-{id}"),
            name: name.to_string(),
            size: None,
            last_modified: None,
            web_url: None,
            is_folder: true,
            parent_drive_id: Some("b!drive".to_string()),
            parent_id: None,
            deleted: false,
        };
        (channel, folder)
    }

    fn item(id: &str, name: &str, is_folder: bool) -> DriveItem {
        DriveItem {
            id: id.to_string(),
            name: name.to_string(),
            size: (!is_folder).then_some(1234),
            last_modified: Some("2026-10-01T08:00:00Z".to_string()),
            web_url: Some(format!("https://contoso.sharepoint.com/{name}")),
            is_folder,
            parent_drive_id: Some("b!drive".to_string()),
            parent_id: Some("root-c1".to_string()),
            deleted: false,
        }
    }

    #[test]
    fn single_channel_is_flattened_onto_the_team() {
        let nodes = map_team(&team(), &[channel("c1", "General")]);

        let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(ids, ["teams:t1", "teams:msg:c1"]);
        assert_eq!(nodes[0].kind, NodeKind::Team);
        assert_eq!(nodes[0].parent, None);
        assert_eq!(nodes[0].name, "Algorithms");
        assert_eq!(nodes[0].remote_id, "b!drive/root-c1");
        assert_eq!(nodes[0].modname, None);
        assert_eq!(nodes[0].web_url, None);

        assert_eq!(nodes[1].kind, NodeKind::Module);
        assert_eq!(nodes[1].modname.as_deref(), Some("messages"));
        assert_eq!(nodes[1].parent.as_deref(), Some("teams:t1"));
        assert_eq!(nodes[1].name, "Messages (General)");
        assert_eq!(nodes[1].remote_id, "t1/c1");
        assert_eq!(
            nodes[1].web_url.as_deref(),
            Some("https://teams.microsoft.com/l/channel/c1")
        );
        assert!(nodes.iter().all(|n| n.source == SourceKind::Teams));
    }

    #[test]
    fn several_channels_get_their_own_nodes() {
        let nodes = map_team(&team(), &[channel("c1", "General"), channel("c2", "Labs")]);

        let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "teams:t1",
                "teams:chan:c1",
                "teams:msg:c1",
                "teams:chan:c2",
                "teams:msg:c2"
            ]
        );
        // A multi-channel team has no files of its own, so no drive.
        assert_eq!(nodes[0].remote_id, "t1");

        assert_eq!(nodes[1].kind, NodeKind::Channel);
        assert_eq!(nodes[1].parent.as_deref(), Some("teams:t1"));
        assert_eq!(nodes[1].name, "General");
        assert_eq!(nodes[1].remote_id, "b!drive/root-c1");
        assert_eq!(
            nodes[1].web_url.as_deref(),
            Some("https://teams.microsoft.com/l/channel/c1")
        );
        assert_eq!(nodes[2].parent.as_deref(), Some("teams:chan:c1"));
        assert_eq!(nodes[2].name, "Messages (General)");
        assert_eq!(nodes[3].remote_id, "b!drive/root-c2");
        assert_eq!(nodes[4].parent.as_deref(), Some("teams:chan:c2"));
        assert_eq!(nodes[4].remote_id, "t1/c2");
    }

    #[test]
    fn team_without_channels_is_just_the_team() {
        let nodes = map_team(&team(), &[]);
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].remote_id, "t1");
    }

    #[test]
    fn maps_a_folder() {
        let mapped = map_item("teams:t1", "b!drive", &item("01DIR", "Week 1", true));
        let Some(Mapped::Folder(node)) = mapped else {
            panic!("expected a folder, got {mapped:?}");
        };
        assert_eq!(node.id, "teams:item:01DIR");
        assert_eq!(node.kind, NodeKind::Folder);
        assert_eq!(node.parent.as_deref(), Some("teams:t1"));
        assert_eq!(node.name, "Week 1");
        assert_eq!(node.remote_id, "b!drive/01DIR");
        assert_eq!(
            node.web_url.as_deref(),
            Some("https://contoso.sharepoint.com/Week 1")
        );
        assert_eq!(node.modname, None);
    }

    #[test]
    fn maps_a_file() {
        let mapped = map_item(
            "teams:item:01DIR",
            "b!drive",
            &item("01ABC", "slides.pdf", false),
        );
        let Some(Mapped::File(file)) = mapped else {
            panic!("expected a file, got {mapped:?}");
        };
        assert_eq!(file.id, "teams:file:01ABC");
        assert_eq!(file.node, "teams:item:01DIR");
        assert_eq!(file.name, "slides.pdf");
        assert_eq!(file.size, Some(1234));
        assert_eq!(file.mtime, Some(1_790_841_600));
        assert_eq!(file.mime, None);
        assert_eq!(file.remote_id, "b!drive/01ABC");
        assert_eq!(
            file.download_url,
            "https://graph.microsoft.com/v1.0/drives/b!drive/items/01ABC/content"
        );
        assert_eq!(
            file.web_url.as_deref(),
            Some("https://contoso.sharepoint.com/slides.pdf")
        );
    }

    #[test]
    fn deleted_items_are_skipped() {
        let mut gone = item("01GONE", "old.docx", false);
        gone.deleted = true;
        assert_eq!(map_item("teams:t1", "b!drive", &gone), None);
    }

    #[test]
    fn parses_graph_timestamps() {
        assert_eq!(rfc3339_to_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(rfc3339_to_unix("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(rfc3339_to_unix("2026-10-01T08:00:00Z"), Some(1_790_841_600));
        assert_eq!(
            rfc3339_to_unix("2026-10-19T12:34:56.789Z"),
            Some(1_792_413_296)
        );
        // A leap day, and the last second of it.
        assert_eq!(rfc3339_to_unix("2024-02-29T23:59:59Z"), Some(1_709_251_199));
    }

    #[test]
    fn rejects_other_shapes() {
        assert_eq!(rfc3339_to_unix(""), None);
        assert_eq!(rfc3339_to_unix("2026-10-01T08:00:00"), None);
        assert_eq!(rfc3339_to_unix("2026-10-01T08:00:00+02:00"), None);
        assert_eq!(rfc3339_to_unix("2026-10-01 08:00:00Z"), None);
        assert_eq!(rfc3339_to_unix("2026-13-01T08:00:00Z"), None);
        assert_eq!(rfc3339_to_unix("2026-10-01T24:00:00Z"), None);
        assert_eq!(rfc3339_to_unix("2026-10-01T08:00:00:00Z"), None);
        assert_eq!(rfc3339_to_unix("1969-12-31T23:59:59Z"), None);
        assert_eq!(rfc3339_to_unix("abcd-ef-ghTij:kl:mnZ"), None);
    }
}
