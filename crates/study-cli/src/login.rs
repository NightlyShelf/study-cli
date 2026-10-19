//! `study login moodle [URL]` — the mobile-app launch flow (SPEC §5.1).

use std::io::{self, BufRead, Write};

use anyhow::{Context, Result, bail};
use study_core::secrets;
use study_moodle::MoodleClient;
use study_moodle::launch;

use crate::app;

pub async fn moodle(url: Option<String>) -> Result<()> {
    let mut config = app::load_config()?;
    let url = match url {
        Some(url) => url,
        None if !config.moodle.url.is_empty() => config.moodle.url.clone(),
        None => bail!("which Moodle? run `study login moodle https://moodle.example.edu`"),
    };
    let url = url.trim_end_matches('/').to_string();

    let http = reqwest::Client::new();
    let site = launch::public_config(&http, &url)
        .await
        .with_context(|| format!("checking {url}"))?;
    println!("Site: {}", site.sitename);

    config.moodle.url = url.clone();
    app::save_config(&config)?;

    let passport = launch::new_passport();
    let launch_url = launch::launch_url(&url, &passport);
    println!("Sign in here (opening in your browser):\n  {launch_url}");
    // The URL is printed above, so a browser that cannot be opened is no loss.
    let _ = open::that(&launch_url);

    let line = prompt(
        "After signing in, paste the moodlemobile://token=… link (or just the token string) here: ",
    )?;
    let token = launch::token_from_launch(&url, &passport, &line)?;

    let client = MoodleClient::new(&url, &token);
    let info = client.site_info().await.context("validating the token")?;
    secrets::set("moodle_token", &token).context("storing the Moodle token")?;
    println!("Logged in to {} as {}.", info.sitename, info.fullname);
    Ok(())
}

fn prompt(message: &str) -> Result<String> {
    eprint!("{message}");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    Ok(line.trim().to_string())
}
