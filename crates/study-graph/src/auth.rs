//! Device-code login and token refresh against Entra ID (SPEC §5.2).
//!
//! Both talk to `<authority>/oauth2/v2.0/{devicecode,token}` with the
//! Microsoft Teams first-party client and the `.default` scope. That pairing
//! is settled; nothing here probes other clients or scopes.

use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::Value;

use crate::error::{Error, Result};

/// `.default` yields whatever Graph permissions the client is preauthorized
/// for; `offline_access` is what makes a refresh token come back.
pub const SCOPE: &str = "https://graph.microsoft.com/.default offline_access";

const DEVICE_CODE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

/// What `/devicecode` hands back: show `user_code` + `verification_uri`, then
/// poll with `device_code`.
#[derive(Debug, Deserialize)]
pub struct DeviceLogin {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Seconds between polls.
    #[serde(default)]
    pub interval: u64,
    /// Seconds until the device code stops working.
    #[serde(default)]
    pub expires_in: u64,
}

/// A `/token` reply. Microsoft returns errors with HTTP 400 and the same
/// shape, so success and failure live in one struct.
#[derive(Debug, Default, Deserialize)]
pub struct TokenResponse {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub refresh_token: Option<String>,
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

pub fn parse_token_response(value: Value) -> Result<TokenResponse> {
    Ok(serde_json::from_value(value)?)
}

/// Asks for a device code. The caller shows the returned code and url.
pub async fn start_device_login(
    http: &reqwest::Client,
    authority: &str,
    client_id: &str,
) -> Result<DeviceLogin> {
    let form = [("client_id", client_id), ("scope", SCOPE)];
    let url = endpoint(authority, "devicecode");
    let value: Value = http.post(url).form(&form).send().await?.json().await?;
    if let Some(error) = auth_error(&value) {
        return Err(error);
    }
    Ok(serde_json::from_value(value)?)
}

/// Polls `/token` until the user has signed in and returns the refresh token.
pub async fn finish_device_login(
    http: &reqwest::Client,
    authority: &str,
    client_id: &str,
    login: &DeviceLogin,
) -> Result<String> {
    let url = endpoint(authority, "token");
    let form = [
        ("grant_type", DEVICE_CODE_GRANT),
        ("client_id", client_id),
        ("device_code", login.device_code.as_str()),
    ];
    let deadline = Instant::now() + Duration::from_secs(login.expires_in);
    // Microsoft always says 5; the floor only guards against a hot loop.
    let mut interval = login.interval.max(1);

    loop {
        tokio::time::sleep(Duration::from_secs(interval)).await;
        if Instant::now() > deadline {
            return Err(Error::Login("timed out".to_string()));
        }

        let value: Value = http.post(&url).form(&form).send().await?.json().await?;
        let token = parse_token_response(value)?;
        match token.error.as_deref() {
            None => return refresh_token_of(token),
            Some("authorization_pending") => {}
            Some("slow_down") => interval += 5,
            Some("expired_token") => return Err(Error::Login("timed out".to_string())),
            Some("authorization_declined") => {
                return Err(Error::Login("declined".to_string()));
            }
            Some(code) => {
                return Err(Error::Auth {
                    code: code.to_string(),
                    description: token.error_description.unwrap_or_default(),
                });
            }
        }
    }
}

/// Trades a refresh token for an access token (plus, usually, a rotated
/// refresh token). A dead grant becomes [`Error::AuthExpired`].
pub async fn refresh_access_token(
    http: &reqwest::Client,
    authority: &str,
    client_id: &str,
    refresh_token: &str,
) -> Result<TokenResponse> {
    let form = [
        ("grant_type", "refresh_token"),
        ("client_id", client_id),
        ("refresh_token", refresh_token),
        ("scope", SCOPE),
    ];
    let url = endpoint(authority, "token");
    let value: Value = http.post(url).form(&form).send().await?.json().await?;
    let token = parse_token_response(value)?;
    match token.error.as_deref() {
        None => Ok(token),
        Some("invalid_grant" | "interaction_required") => Err(Error::AuthExpired(
            token.error_description.unwrap_or_default(),
        )),
        Some(code) => Err(Error::Auth {
            code: code.to_string(),
            description: token.error_description.unwrap_or_default(),
        }),
    }
}

/// `authority` is the config value (`https://login.microsoftonline.com/organizations`);
/// the SPEC spells it with the `/oauth2/v2.0` suffix, so both are accepted.
fn endpoint(authority: &str, name: &str) -> String {
    let base = authority
        .trim_end_matches('/')
        .trim_end_matches("/oauth2/v2.0");
    format!("{base}/oauth2/v2.0/{name}")
}

fn refresh_token_of(token: TokenResponse) -> Result<String> {
    token.refresh_token.ok_or_else(|| {
        Error::Login("token response carries no refresh_token (is offline_access missing?)".into())
    })
}

/// Reads an `{"error","error_description"}` body, if that is what came back.
fn auth_error(value: &Value) -> Option<Error> {
    let code = value.get("error")?.as_str()?.to_string();
    let description = value
        .get("error_description")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    Some(Error::Auth { code, description })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn parse_token_response_success() {
        let token = parse_token_response(json!({
            "token_type": "Bearer",
            "scope": "https://graph.microsoft.com/.default",
            "expires_in": 3599,
            "access_token": "AT",
            "refresh_token": "RT"
        }))
        .unwrap();
        assert_eq!(token.access_token.as_deref(), Some("AT"));
        assert_eq!(token.refresh_token.as_deref(), Some("RT"));
        assert_eq!(token.expires_in, Some(3599));
        assert!(token.error.is_none());
    }

    #[test]
    fn parse_token_response_pending() {
        let token = parse_token_response(json!({
            "error": "authorization_pending",
            "error_description": "AADSTS70016: Pending end-user authorization.",
            "error_codes": [70016]
        }))
        .unwrap();
        assert_eq!(token.error.as_deref(), Some("authorization_pending"));
        assert!(token.access_token.is_none());
    }

    #[test]
    fn parse_token_response_slow_down() {
        let token = parse_token_response(json!({"error": "slow_down"})).unwrap();
        assert_eq!(token.error.as_deref(), Some("slow_down"));
        assert!(token.error_description.is_none());
    }

    #[test]
    fn parse_token_response_error_keeps_description() {
        let token = parse_token_response(json!({
            "error": "invalid_grant",
            "error_description": "AADSTS50173: The provided grant has expired."
        }))
        .unwrap();
        assert_eq!(token.error.as_deref(), Some("invalid_grant"));
        assert_eq!(
            token.error_description.as_deref(),
            Some("AADSTS50173: The provided grant has expired.")
        );
    }

    #[test]
    fn endpoint_accepts_both_authority_spellings() {
        let expected = "https://login.microsoftonline.com/organizations/oauth2/v2.0/token";
        assert_eq!(
            endpoint("https://login.microsoftonline.com/organizations", "token"),
            expected
        );
        assert_eq!(
            endpoint("https://login.microsoftonline.com/organizations/", "token"),
            expected
        );
        assert_eq!(
            endpoint(
                "https://login.microsoftonline.com/organizations/oauth2/v2.0",
                "token"
            ),
            expected
        );
    }

    #[test]
    fn auth_error_reads_oauth_error_body() {
        let value = json!({"error": "invalid_client", "error_description": "AADSTS7000218"});
        match auth_error(&value) {
            Some(Error::Auth { code, description }) => {
                assert_eq!(code, "invalid_client");
                assert_eq!(description, "AADSTS7000218");
            }
            other => panic!("expected Auth error, got {other:?}"),
        }
        assert!(auth_error(&json!({"device_code": "x"})).is_none());
    }
}
