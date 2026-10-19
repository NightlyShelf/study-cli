//! `study login moodle [URL]` — the mobile-app launch flow (SPEC §5.1).

use std::fs;
use std::io::{self, BufRead, Write};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use study_core::secrets;
use study_moodle::MoodleClient;
use study_moodle::launch;

use crate::{app, handler};

const POLL: Duration = Duration::from_millis(500);
const LOGIN_TIMEOUT: Duration = Duration::from_secs(5 * 60);

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

    // A leftover from an aborted login would be taken for this one's redirect.
    let _ = fs::remove_file(handler::callback_path());
    if let Err(err) = handler::install_handler() {
        eprintln!("warning: could not register the moodlemobile:// handler ({err:#})");
    }

    let passport = launch::new_passport();
    let launch_url = launch::launch_url(&url, &passport);
    println!("Sign in here (opening in your browser):\n  {launch_url}");
    // The URL is printed above, so a browser that cannot be opened is no loss.
    let _ = open::that(&launch_url);

    eprint!(
        "Waiting for the browser to hand the login back (or paste the moodlemobile://token=… link here): "
    );
    io::stderr().flush()?;
    let redirect = wait_for_redirect()?;
    let token = launch::token_from_launch(&url, &passport, &redirect)?;

    let client = MoodleClient::new(&url, &token);
    let info = client.site_info().await.context("validating the token")?;
    secrets::set("moodle_token", &token).context("storing the Moodle token")?;
    println!("Logged in to {} as {}.", info.sitename, info.fullname);
    Ok(())
}

/// The redirect, from whichever arrives first: a line pasted on stdin or
/// the callback file the scheme handler writes. Stdin is read on a thread
/// because a blocking read cannot be polled alongside the file.
fn wait_for_redirect() -> Result<String> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let mut line = String::new();
        if io::stdin().lock().read_line(&mut line).is_ok() {
            let _ = tx.send(line);
        }
    });

    let callback = handler::callback_path();
    let deadline = Instant::now() + LOGIN_TIMEOUT;
    while Instant::now() < deadline {
        match rx.recv_timeout(POLL) {
            Ok(line) => return Ok(line.trim().to_string()),
            Err(RecvTimeoutError::Timeout) => {}
            // Stdin is closed (piped); only the handler can deliver now.
            Err(RecvTimeoutError::Disconnected) => thread::sleep(POLL),
        }
        if let Ok(url) = fs::read_to_string(&callback) {
            let _ = fs::remove_file(&callback);
            return Ok(url.trim().to_string());
        }
    }
    bail!("no login received within 5 minutes — run `study login moodle` again")
}
