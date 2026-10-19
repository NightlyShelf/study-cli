use serde_json::Value;

use crate::error::{Error, Result};

pub struct MoodleClient {
    /// Site root without a trailing slash, e.g. `https://moodle.example.edu`.
    pub base_url: String,
    pub token: String,
    http: reqwest::Client,
}

impl MoodleClient {
    pub fn new(base_url: &str, token: &str) -> MoodleClient {
        MoodleClient {
            base_url: base_url.trim_end_matches('/').to_string(),
            token: token.to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Calls a web-service function. `params` must already be flattened the way
    /// Moodle expects (`courseids[0]=12`); see [`list_param`].
    pub async fn call(&self, function: &str, params: &[(&str, String)]) -> Result<Value> {
        let mut form: Vec<(&str, String)> = vec![
            ("wstoken", self.token.clone()),
            ("wsfunction", function.to_string()),
            ("moodlewsrestformat", "json".to_string()),
        ];
        form.extend(params.iter().cloned());

        let url = format!("{}/webservice/rest/server.php", self.base_url);
        let response = self.http.post(url).form(&form).send().await?;
        let value: Value = response.error_for_status()?.json().await?;
        check_response(value)
    }

    /// Turns a `pluginfile.php` url from the API into a downloadable one.
    pub fn file_url(&self, fileurl: &str) -> String {
        let separator = if fileurl.contains('?') { '&' } else { '?' };
        format!("{fileurl}{separator}token={}", self.token)
    }

    /// Starts a download of a `pluginfile.php` url; the caller streams the body.
    pub async fn download(&self, fileurl: &str) -> Result<reqwest::Response> {
        let response = self.http.get(self.file_url(fileurl)).send().await?;
        Ok(response.error_for_status()?)
    }
}

/// Flattens a list into Moodle's `name[0]`, `name[1]`, … form parameters.
pub fn list_param(name: &str, values: &[String]) -> Vec<(String, String)> {
    values
        .iter()
        .enumerate()
        .map(|(i, value)| (format!("{name}[{i}]"), value.clone()))
        .collect()
}

/// Moodle reports errors with HTTP 200 and an `exception` object, so the body
/// has to be inspected before it can be trusted.
fn check_response(value: Value) -> Result<Value> {
    let Some(object) = value.as_object() else {
        return Ok(value);
    };
    if !object.contains_key("exception") {
        return Ok(value);
    }

    let text = |key: &str| {
        object
            .get(key)
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let code = text("errorcode");
    let message = text("message");
    if code == "invalidtoken" || code == "accessexception_token_expired" {
        return Err(Error::TokenExpired(message));
    }
    Err(Error::Moodle { code, message })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn new_trims_trailing_slash() {
        let client = MoodleClient::new("https://moodle.test/", "tok");
        assert_eq!(client.base_url, "https://moodle.test");
    }

    #[test]
    fn file_url_appends_token_with_question_mark() {
        let client = MoodleClient::new("https://moodle.test", "tok");
        let url = client.file_url("https://moodle.test/webservice/pluginfile.php/1/a.pdf");
        assert_eq!(
            url,
            "https://moodle.test/webservice/pluginfile.php/1/a.pdf?token=tok"
        );
    }

    #[test]
    fn file_url_appends_token_with_ampersand() {
        let client = MoodleClient::new("https://moodle.test", "tok");
        let url = client
            .file_url("https://moodle.test/webservice/pluginfile.php/1/a.pdf?forcedownload=1");
        assert_eq!(
            url,
            "https://moodle.test/webservice/pluginfile.php/1/a.pdf?forcedownload=1&token=tok"
        );
    }

    #[test]
    fn list_param_indexes_values() {
        let params = list_param("courseids", &["12".to_string(), "34".to_string()]);
        assert_eq!(
            params,
            vec![
                ("courseids[0]".to_string(), "12".to_string()),
                ("courseids[1]".to_string(), "34".to_string()),
            ]
        );
    }

    #[test]
    fn check_response_detects_expired_token() {
        let value = json!({
            "exception": "moodle_exception",
            "errorcode": "invalidtoken",
            "message": "Invalid token - token not found"
        });
        match check_response(value) {
            Err(Error::TokenExpired(message)) => {
                assert_eq!(message, "Invalid token - token not found");
            }
            other => panic!("expected TokenExpired, got {other:?}"),
        }
    }

    #[test]
    fn check_response_detects_other_exception() {
        let value = json!({
            "exception": "webservice_access_exception",
            "errorcode": "accessexception",
            "message": "Access control exception"
        });
        match check_response(value) {
            Err(Error::Moodle { code, message }) => {
                assert_eq!(code, "accessexception");
                assert_eq!(message, "Access control exception");
            }
            other => panic!("expected Moodle error, got {other:?}"),
        }
    }

    #[test]
    fn check_response_passes_plain_payload() {
        let value = json!([{"id": 1}]);
        assert_eq!(check_response(value.clone()).unwrap(), value);

        let object = json!({"userid": 7, "sitename": "Test"});
        assert_eq!(check_response(object.clone()).unwrap(), object);
    }
}
