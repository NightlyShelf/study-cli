//! The mobile-app login flow (SPEC §5.1): open `launch.php` in a browser,
//! receive a `moodlemobile://token=<base64>` redirect, decode the token.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD_PAD_INDIFFERENT, URL_SAFE_PAD_INDIFFERENT};
use md5::{Digest, Md5};
use serde::Deserialize;

use crate::error::{Error, Result};

/// Site settings readable without a token, from `tool_mobile_get_public_config`.
#[derive(Debug, Deserialize)]
pub struct PublicConfig {
    #[serde(default)]
    pub wwwroot: String,
    #[serde(default)]
    pub sitename: String,
    /// 1 when the mobile web service is enabled.
    #[serde(default)]
    pub enablemobilewebservice: u8,
    #[serde(default)]
    pub typeoflogin: u8,
    #[serde(default)]
    pub launchurl: String,
    #[serde(default)]
    pub identityproviders: Vec<IdentityProvider>,
}

#[derive(Debug, Deserialize)]
pub struct IdentityProvider {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub url: String,
}

/// The passport is a random string echoed into the launch signature so the
/// token we receive can be tied to the launch we started.
pub fn new_passport() -> String {
    let bytes: [u8; 16] = rand::random();
    hex(&bytes)
}

pub fn launch_url(base_url: &str, passport: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let mut url = url::Url::parse(&format!("{base}/admin/tool/mobile/launch.php"))
        .expect("base url is valid");
    url.query_pairs_mut()
        .append_pair("service", "moodle_mobile_app")
        .append_pair("passport", passport)
        .append_pair("urlscheme", "moodlemobile");
    url.to_string()
}

/// Extracts the web-service token from the redirect Moodle sends after login.
/// Accepts the full `moodlemobile://token=<base64>` url or the bare base64.
pub fn token_from_launch(base_url: &str, passport: &str, redirect: &str) -> Result<String> {
    let encoded = match redirect.split_once("token=") {
        Some((_, rest)) => rest,
        None => redirect,
    };
    let encoded = encoded.trim();
    if encoded.is_empty() {
        return Err(Error::Launch("redirect contains no token".to_string()));
    }

    let decoded = STANDARD_PAD_INDIFFERENT
        .decode(encoded)
        .or_else(|_| URL_SAFE_PAD_INDIFFERENT.decode(encoded))
        .map_err(|_| Error::Launch("token is not valid base64".to_string()))?;
    let decoded = String::from_utf8(decoded)
        .map_err(|_| Error::Launch("decoded token is not utf-8".to_string()))?;

    let mut parts = decoded.split(":::");
    let (Some(signature), Some(token)) = (parts.next(), parts.next()) else {
        return Err(Error::Launch(
            "decoded token has no signature:::token parts".to_string(),
        ));
    };

    if signature != launch_signature(base_url, passport) {
        return Err(Error::Launch(
            "launch signature does not match the passport".to_string(),
        ));
    }
    Ok(token.to_string())
}

/// Fetches the public site config through the no-login AJAX endpoint and
/// fails early when the mobile web service is disabled.
pub async fn public_config(http: &reqwest::Client, base_url: &str) -> Result<PublicConfig> {
    let base = base_url.trim_end_matches('/');
    let args = r#"[{"index":0,"methodname":"tool_mobile_get_public_config","args":{}}]"#;
    let mut url = url::Url::parse(&format!("{base}/lib/ajax/service-nologin.php"))
        .map_err(|e| Error::Launch(format!("invalid site url: {e}")))?;
    url.query_pairs_mut().append_pair("args", args);

    let response = http.get(url).send().await?.error_for_status()?;
    let mut replies: Vec<AjaxReply> = response.json().await?;
    let Some(reply) = replies.pop() else {
        return Err(Error::Launch("empty public config response".to_string()));
    };
    if reply.error {
        return Err(Error::Launch(reply.exception.message));
    }

    let config: PublicConfig = serde_json::from_value(reply.data)?;
    if config.enablemobilewebservice != 1 {
        return Err(Error::Launch(
            "mobile web service is disabled on this site".to_string(),
        ));
    }
    Ok(config)
}

#[derive(Deserialize)]
struct AjaxReply {
    #[serde(default)]
    error: bool,
    #[serde(default)]
    exception: AjaxException,
    #[serde(default)]
    data: serde_json::Value,
}

#[derive(Default, Deserialize)]
struct AjaxException {
    #[serde(default)]
    message: String,
}

fn launch_signature(base_url: &str, passport: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let digest = Md5::digest(format!("{base}{passport}"));
    hex(&digest)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use base64::engine::general_purpose::{STANDARD, STANDARD_NO_PAD};

    use super::*;

    const BASE: &str = "https://moodle.test";
    const PASSPORT: &str = "0123456789abcdef0123456789abcdef";

    fn encode_launch(base: &str, passport: &str, token: &str) -> String {
        let plain = format!("{}:::{token}:::PRIVATE", launch_signature(base, passport));
        STANDARD.encode(plain)
    }

    #[test]
    fn passport_is_32_hex_chars() {
        let passport = new_passport();
        assert_eq!(passport.len(), 32);
        assert!(passport.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(passport, new_passport());
    }

    #[test]
    fn launch_url_has_expected_shape() {
        let url = launch_url("https://moodle.test/", "abc def");
        assert_eq!(
            url,
            "https://moodle.test/admin/tool/mobile/launch.php?service=moodle_mobile_app&passport=abc+def&urlscheme=moodlemobile"
        );
    }

    #[test]
    fn token_from_full_redirect() {
        let redirect = format!(
            "moodlemobile://token={}",
            encode_launch(BASE, PASSPORT, "TOKEN")
        );
        assert_eq!(
            token_from_launch(BASE, PASSPORT, &redirect).unwrap(),
            "TOKEN"
        );
    }

    #[test]
    fn token_from_bare_base64() {
        let encoded = encode_launch(BASE, PASSPORT, "TOKEN");
        assert_eq!(
            token_from_launch(BASE, PASSPORT, &encoded).unwrap(),
            "TOKEN"
        );
    }

    #[test]
    fn token_tolerates_missing_padding_and_whitespace() {
        let plain = format!("{}:::TOKEN", launch_signature(BASE, PASSPORT));
        let encoded = STANDARD_NO_PAD.encode(plain);
        assert!(!encoded.ends_with('='));
        let redirect = format!(" moodlemobile://token={encoded}\n");
        assert_eq!(
            token_from_launch(BASE, PASSPORT, &redirect).unwrap(),
            "TOKEN"
        );
    }

    #[test]
    fn token_accepts_base_url_with_trailing_slash() {
        let encoded = encode_launch(BASE, PASSPORT, "TOKEN");
        let with_slash = format!("{BASE}/");
        assert_eq!(
            token_from_launch(&with_slash, PASSPORT, &encoded).unwrap(),
            "TOKEN"
        );
    }

    #[test]
    fn token_rejects_signature_mismatch() {
        let encoded = encode_launch(BASE, "another-passport", "TOKEN");
        match token_from_launch(BASE, PASSPORT, &encoded) {
            Err(Error::Launch(message)) => {
                assert_eq!(message, "launch signature does not match the passport");
            }
            other => panic!("expected Launch error, got {other:?}"),
        }
    }

    #[test]
    fn token_rejects_garbage() {
        assert!(matches!(
            token_from_launch(BASE, PASSPORT, "not base64!!"),
            Err(Error::Launch(_))
        ));
        assert!(matches!(
            token_from_launch(BASE, PASSPORT, "moodlemobile://token="),
            Err(Error::Launch(_))
        ));
    }
}
