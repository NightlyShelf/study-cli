//! Shared plumbing: config and index on disk, the Moodle client, auto-sync.

use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use study_core::config::{self, Config};
use study_core::index::{self, Index};
use study_core::secrets;
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
