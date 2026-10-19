//! File upload into the user's draft area via `webservice/upload.php`.
//!
//! `core_files_upload` takes the file as base64 inside the JSON body, which
//! is hopeless for big files, so the multipart endpoint is used instead.

use std::path::Path;

use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use serde_json::Value;

use crate::client::MoodleClient;
use crate::error::{Error, Result};

#[derive(Deserialize)]
struct Uploaded {
    itemid: u64,
}

#[derive(Deserialize)]
struct UploadError {
    #[serde(default)]
    error: String,
    #[serde(default)]
    errorcode: String,
}

impl MoodleClient {
    /// Uploads one file into the user's draft area and returns the draft `itemid`.
    /// Pass `itemid = Some(..)` to add a second file to the same draft area.
    pub async fn upload_file(&self, path: &Path, itemid: Option<u64>) -> Result<u64> {
        let bytes = tokio::fs::read(path).await?;
        let filename = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let form = Form::new()
            .text("token", self.token.clone())
            .text("filearea", "draft")
            .text("itemid", itemid.unwrap_or(0).to_string())
            .part("file_1", Part::bytes(bytes).file_name(filename));

        let url = format!("{}/webservice/upload.php", self.base_url);
        let response = self.http.post(url).multipart(form).send().await?;
        let value: Value = response.error_for_status()?.json().await?;
        parse_upload(value)
    }
}

/// `upload.php` answers with an array of stored files on success and a bare
/// error object (no `exception` key) on failure, so `check_response` does
/// not apply.
fn parse_upload(value: Value) -> Result<u64> {
    if value.is_object() {
        let failure: UploadError = serde_json::from_value(value)?;
        if failure.errorcode == "invalidtoken" {
            return Err(Error::TokenExpired(failure.error));
        }
        return Err(Error::Moodle {
            code: failure.errorcode,
            message: failure.error,
        });
    }

    let files: Vec<Uploaded> = serde_json::from_value(value)?;
    let Some(first) = files.first() else {
        return Err(Error::Moodle {
            code: "nofilesuploaded".to_string(),
            message: "upload.php returned no files".to_string(),
        });
    };
    Ok(first.itemid)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parse_upload_returns_itemid() {
        let value = json!([{
            "component": "user", "contextid": 123, "userid": 5, "filearea": "draft",
            "filename": "hw5.pdf", "filepath": "/", "itemid": 987654,
            "license": "allrightsreserved", "author": "Ada", "source": ""
        }]);
        assert_eq!(parse_upload(value).unwrap(), 987654);
    }

    #[test]
    fn parse_upload_maps_error_object() {
        let value = json!({
            "error": "File area not allowed",
            "errorcode": "invalidfilearea",
            "stacktrace": null,
            "debuginfo": null
        });
        match parse_upload(value) {
            Err(Error::Moodle { code, message }) => {
                assert_eq!(code, "invalidfilearea");
                assert_eq!(message, "File area not allowed");
            }
            other => panic!("expected Moodle error, got {other:?}"),
        }
    }

    #[test]
    fn parse_upload_detects_expired_token() {
        let value =
            json!({"error": "Invalid token - token not found", "errorcode": "invalidtoken"});
        match parse_upload(value) {
            Err(Error::TokenExpired(message)) => {
                assert_eq!(message, "Invalid token - token not found");
            }
            other => panic!("expected TokenExpired, got {other:?}"),
        }
    }

    #[test]
    fn parse_upload_rejects_empty_array() {
        match parse_upload(json!([])) {
            Err(Error::Moodle { code, .. }) => assert_eq!(code, "nofilesuploaded"),
            other => panic!("expected Moodle error, got {other:?}"),
        }
    }
}
