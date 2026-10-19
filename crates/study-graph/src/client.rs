use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use reqwest::StatusCode;
use serde_json::Value;

use crate::auth;
use crate::error::{Error, Result};

pub const GRAPH_BASE: &str = "https://graph.microsoft.com/v1.0";

/// Access tokens are treated as expired this long before they really are, so
/// a request started just before the deadline does not fail mid-flight.
const EXPIRY_MARGIN: Duration = Duration::from_secs(120);

/// Graph client holding the refresh token and a cached access token.
///
/// Both caches sit behind `std::sync::Mutex`, never `tokio`'s: the locks only
/// wrap a clone or a store, never an `.await`, and the synchronous
/// [`GraphClient::refresh_token`] getter needs no executor. Two tasks
/// refreshing at once is harmless — Entra keeps the previous refresh token
/// valid, so the last writer simply wins.
#[derive(Clone)]
pub struct GraphClient {
    /// E.g. `https://login.microsoftonline.com/organizations`.
    pub authority: String,
    pub client_id: String,
    pub(crate) http: reqwest::Client,
    refresh_token: Arc<Mutex<String>>,
    access: Arc<Mutex<Option<(String, Instant)>>>,
}

impl GraphClient {
    pub fn new(authority: &str, client_id: &str, refresh_token: &str) -> GraphClient {
        GraphClient {
            authority: authority.to_string(),
            client_id: client_id.to_string(),
            http: reqwest::Client::new(),
            refresh_token: Arc::new(Mutex::new(refresh_token.to_string())),
            access: Arc::new(Mutex::new(None)),
        }
    }

    /// The cached access token, or a fresh one from the refresh token.
    pub async fn access_token(&self) -> Result<String> {
        if let Some(token) = self.cached_access_token() {
            return Ok(token);
        }
        self.refresh().await
    }

    /// The current refresh token. Entra rotates it on every refresh, so the
    /// caller persists this value after any call, not the one it started with.
    pub fn refresh_token(&self) -> String {
        self.refresh_token.lock().expect("mutex poisoned").clone()
    }

    /// GETs an absolute url or a path under [`GRAPH_BASE`] as JSON.
    pub async fn get(&self, url_or_path: &str) -> Result<Value> {
        let url = absolute(url_or_path);
        let response = self.send(self.http.get(url)).await?;
        json_body(response).await
    }

    /// GETs a collection, following `@odata.nextLink` until the last page.
    pub async fn get_all(&self, path: &str) -> Result<Vec<Value>> {
        let mut items = Vec::new();
        let mut next = Some(absolute(path));
        while let Some(url) = next {
            let page = self.get(&url).await?;
            next = merge_page(&mut items, page);
        }
        Ok(items)
    }

    /// Sends `request` with a Bearer token. On 401 the cached token is dropped
    /// and the request is retried once with a fresh one.
    pub(crate) async fn send(&self, request: reqwest::RequestBuilder) -> Result<reqwest::Response> {
        // `try_clone` is `None` only for streaming bodies, which nothing here uses.
        let retry = request.try_clone();
        let token = self.access_token().await?;
        let response = request.bearer_auth(token).send().await?;
        if response.status() != StatusCode::UNAUTHORIZED {
            return Ok(response);
        }
        let Some(retry) = retry else {
            return Ok(response);
        };

        *self.access.lock().expect("mutex poisoned") = None;
        let token = self.access_token().await?;
        Ok(retry.bearer_auth(token).send().await?)
    }

    fn cached_access_token(&self) -> Option<String> {
        let access = self.access.lock().expect("mutex poisoned");
        let (token, expires) = access.as_ref()?;
        if Instant::now() + EXPIRY_MARGIN >= *expires {
            return None;
        }
        Some(token.clone())
    }

    async fn refresh(&self) -> Result<String> {
        let refresh_token = self.refresh_token();
        let token = auth::refresh_access_token(
            &self.http,
            &self.authority,
            &self.client_id,
            &refresh_token,
        )
        .await?;
        let Some(access_token) = token.access_token else {
            return Err(Error::Auth {
                code: "no_access_token".to_string(),
                description: "token response carries no access_token".to_string(),
            });
        };

        if let Some(rotated) = token.refresh_token {
            *self.refresh_token.lock().expect("mutex poisoned") = rotated;
        }
        let expires = Instant::now() + Duration::from_secs(token.expires_in.unwrap_or(3600));
        *self.access.lock().expect("mutex poisoned") = Some((access_token.clone(), expires));
        Ok(access_token)
    }
}

/// Paths are relative to [`GRAPH_BASE`]; `nextLink`/`deltaLink` urls from
/// Graph are already absolute.
pub(crate) fn absolute(url_or_path: &str) -> String {
    if url_or_path.starts_with("https://") {
        return url_or_path.to_string();
    }
    format!("{GRAPH_BASE}/{}", url_or_path.trim_start_matches('/'))
}

/// Appends a page's `value` array to `items` and returns the next page's url.
pub(crate) fn merge_page(items: &mut Vec<Value>, mut page: Value) -> Option<String> {
    if let Some(values) = page.get_mut("value").and_then(Value::as_array_mut) {
        items.append(values);
    }
    page.get("@odata.nextLink")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Parses a Graph JSON reply, turning non-2xx statuses into [`Error::Graph`].
pub(crate) async fn json_body(response: reqwest::Response) -> Result<Value> {
    if !response.status().is_success() {
        return Err(graph_error(response).await);
    }
    Ok(response.json().await?)
}

pub(crate) async fn graph_error(response: reqwest::Response) -> Error {
    let status = response.status().as_u16();
    match response.text().await {
        Ok(body) => graph_error_from(status, &body),
        Err(error) => Error::Http(error),
    }
}

/// Graph errors look like `{"error":{"code","message"}}`; anything else
/// (a proxy page, an empty body) is passed through as the message.
fn graph_error_from(status: u16, body: &str) -> Error {
    let value: Value = serde_json::from_str(body).unwrap_or_default();
    let error = value.get("error");
    let text = |key: &str| {
        error
            .and_then(|e| e.get(key))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let code = text("code");
    let mut message = text("message");
    if message.is_empty() {
        message = body.trim().to_string();
    }
    Error::Graph {
        status,
        code,
        message,
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn absolute_handles_paths_and_urls() {
        assert_eq!(
            absolute("/me/joinedTeams"),
            "https://graph.microsoft.com/v1.0/me/joinedTeams"
        );
        assert_eq!(
            absolute("me/joinedTeams"),
            "https://graph.microsoft.com/v1.0/me/joinedTeams"
        );
        assert_eq!(
            absolute("https://graph.microsoft.com/v1.0/me?$skiptoken=x"),
            "https://graph.microsoft.com/v1.0/me?$skiptoken=x"
        );
    }

    #[test]
    fn merge_page_concatenates_values_and_returns_next_link() {
        let mut items = Vec::new();
        let next = merge_page(
            &mut items,
            json!({"value": [{"id": "a"}, {"id": "b"}], "@odata.nextLink": "https://graph.microsoft.com/v1.0/x?$skiptoken=1"}),
        );
        assert_eq!(
            next.as_deref(),
            Some("https://graph.microsoft.com/v1.0/x?$skiptoken=1")
        );

        let next = merge_page(&mut items, json!({"value": [{"id": "c"}]}));
        assert!(next.is_none());
        assert_eq!(
            items,
            vec![json!({"id": "a"}), json!({"id": "b"}), json!({"id": "c"})]
        );

        let next = merge_page(&mut items, json!({"id": "not a page"}));
        assert!(next.is_none());
        assert_eq!(items.len(), 3);
    }

    #[test]
    fn graph_error_from_reads_error_object() {
        let body = r#"{"error":{"code":"itemNotFound","message":"The resource could not be found.","innerError":{"date":"2026-10-19"}}}"#;
        match graph_error_from(404, body) {
            Error::Graph {
                status,
                code,
                message,
            } => {
                assert_eq!(status, 404);
                assert_eq!(code, "itemNotFound");
                assert_eq!(message, "The resource could not be found.");
            }
            other => panic!("expected Graph error, got {other:?}"),
        }
    }

    #[test]
    fn graph_error_from_falls_back_to_body_text() {
        match graph_error_from(502, "<html>Bad Gateway</html>\n") {
            Error::Graph {
                status,
                code,
                message,
            } => {
                assert_eq!(status, 502);
                assert_eq!(code, "");
                assert_eq!(message, "<html>Bad Gateway</html>");
            }
            other => panic!("expected Graph error, got {other:?}"),
        }
    }

    #[test]
    fn refresh_token_returns_initial_value() {
        let client = GraphClient::new(
            "https://login.microsoftonline.com/organizations",
            "client",
            "RT",
        );
        assert_eq!(client.refresh_token(), "RT");
        assert!(client.cached_access_token().is_none());
    }
}
