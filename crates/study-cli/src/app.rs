//! Shared plumbing: config and index on disk, the Moodle and Graph clients,
//! auto-sync, and the one place that routes a file download to its source.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use study_core::config::{self, Config};
use study_core::index::{self, File, Index, SourceKind};
use study_core::secrets;
use study_graph::GraphClient;
use study_moodle::MoodleClient;

use crate::sync;

/// Commands auto-sync when the index is older than this (SPEC §6).
pub const STALE_AFTER_SECS: u64 = 15 * 60;

pub fn load_config() -> Result<Config> {
    config::load(&config::default_path()).context("reading config")
}

pub fn save_config(config: &Config) -> Result<()> {
    config::save(config, &config::default_path()).context("saving config")
}

pub fn load_index() -> Result<Index> {
    index::load(&index::default_path()).context("reading index")
}

pub fn save_index(index: &Index) -> Result<()> {
    index::save(index, &index::default_path()).context("saving index")
}

pub fn moodle_client(config: &Config) -> Result<MoodleClient> {
    let token = secrets::get("moodle_token").context("reading the Moodle token")?;
    let Some(token) = token else {
        bail!("not logged in to Moodle — run `study login moodle <url>`");
    };
    if config.moodle.url.is_empty() {
        bail!("no Moodle url in config — run `study login moodle <url>`");
    }
    Ok(MoodleClient::new(&config.moodle.url, &token))
}

pub fn graph_client(config: &Config) -> Result<GraphClient> {
    let token = secrets::get("teams_refresh_token").context("reading the Teams token")?;
    let Some(token) = token else {
        bail!("not logged in to Teams — run `study login teams`");
    };
    Ok(GraphClient::new(
        &config.teams.authority,
        &config.teams.client_id,
        &token,
    ))
}

/// Entra rotates the refresh token on every refresh, so whatever the client
/// holds after a Graph call is the only one still valid next time.
pub fn persist_graph_token(graph: &GraphClient) -> Result<()> {
    secrets::set("teams_refresh_token", &graph.refresh_token()).context("storing the Teams token")
}

/// Starts downloading `file` from whichever source owns it; the caller
/// streams the body (see `download::save`).
pub async fn fetch(
    moodle: Option<&MoodleClient>,
    graph: Option<&GraphClient>,
    file: &File,
) -> Result<reqwest::Response> {
    match source_of(&file.id) {
        SourceKind::Teams => {
            let Some(graph) = graph else {
                bail!("not logged in to Teams — run `study login teams`");
            };
            let (drive, item) = split_remote_id(&file.remote_id)?;
            Ok(graph.download(drive, item).await?)
        }
        SourceKind::Moodle => {
            let Some(moodle) = moodle else {
                bail!("not logged in to Moodle — run `study login moodle <url>`");
            };
            Ok(moodle.download(&file.download_url).await?)
        }
    }
}

/// Which source a node or file id belongs to, from its `<source>:` prefix.
pub fn source_of(id: &str) -> SourceKind {
    if id.starts_with("teams:") {
        return SourceKind::Teams;
    }
    SourceKind::Moodle
}

/// Teams nodes and files store `<driveId>/<itemId>` as their remote id.
pub fn split_remote_id(remote_id: &str) -> Result<(&str, &str)> {
    match remote_id.split_once('/') {
        Some(pair) => Ok(pair),
        None => bail!("'{remote_id}' is not a <drive>/<item> id"),
    }
}

/// The index, synced first when it is missing or older than 15 minutes.
pub async fn fresh_index(config: &Config) -> Result<Index> {
    let index = load_index()?;
    if !index.is_stale(STALE_AFTER_SECS, now()) {
        return Ok(index);
    }
    eprintln!("syncing…");
    sync::sync(config, false).await
}

/// Unix seconds.
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_by_id_prefix() {
        assert_eq!(source_of("teams:file:01ABC"), SourceKind::Teams);
        assert_eq!(source_of("teams:t1"), SourceKind::Teams);
        assert_eq!(source_of("moodle:file:101:/a.pdf"), SourceKind::Moodle);
        assert_eq!(source_of("moodle:1"), SourceKind::Moodle);
        // Anything unprefixed is a Moodle row from before Teams existed.
        assert_eq!(source_of("f1"), SourceKind::Moodle);
    }

    #[test]
    fn splits_drive_and_item() {
        assert_eq!(
            split_remote_id("b!drive/01ITEM").unwrap(),
            ("b!drive", "01ITEM")
        );
        assert!(split_remote_id("t1").is_err());
    }
}
