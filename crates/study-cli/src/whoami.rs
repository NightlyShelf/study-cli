//! `study whoami`: both accounts, token health and the state of the index.

use anyhow::Result;
use study_core::config::Config;
use study_core::index::{self, EventKind, Index, NodeKind, SourceKind};
use study_core::secrets;
use study_graph::GraphClient;
use study_moodle::MoodleClient;

use crate::{app, fmt};

pub async fn run() -> Result<()> {
    let config = app::load_config()?;
    let index = app::load_index()?;

    println!("Moodle");
    moodle(&config).await;
    println!();
    println!("Teams");
    teams(&config).await;
    println!();
    println!("Index");
    index_summary(&index);
    Ok(())
}

async fn moodle(config: &Config) {
    let url = &config.moodle.url;
    println!("  url: {}", if url.is_empty() { "-" } else { url });

    let token = match secrets::get("moodle_token") {
        Ok(Some(token)) => token,
        Ok(None) => {
            println!("  token: none — run study login moodle <url>");
            return;
        }
        Err(err) => {
            println!("  token: error — {err}");
            return;
        }
    };
    if url.is_empty() {
        println!("  token: stored, but no url — run study login moodle <url>");
        return;
    }

    let client = MoodleClient::new(url, &token);
    match client.site_info().await {
        Ok(info) => {
            println!("  token: stored");
            println!(
                "  user: {} ({}) at {}",
                info.fullname, info.username, info.sitename
            );
        }
        Err(study_moodle::Error::TokenExpired(_)) => {
            println!("  token: expired — run study login moodle");
        }
        Err(err) => println!("  token: error — {err}"),
    }
}

async fn teams(config: &Config) {
    println!("  client id: {}", config.teams.client_id);
    println!("  authority: {}", config.teams.authority);

    let token = match secrets::get("teams_refresh_token") {
        Ok(Some(token)) => token,
        Ok(None) => {
            println!("  token: none — run study login teams");
            return;
        }
        Err(err) => {
            println!("  token: error — {err}");
            return;
        }
    };

    let graph = GraphClient::new(&config.teams.authority, &config.teams.client_id, &token);
    match graph.joined_teams().await {
        Ok(teams) => {
            println!("  token: stored");
            println!("  teams: {}", teams.len());
            // The call rotated the refresh token; keep the live one.
            if let Err(err) = app::persist_graph_token(&graph) {
                println!("  token: could not store the rotated token — {err}");
            }
        }
        Err(study_graph::Error::AuthExpired(_)) => {
            println!("  token: expired — run study login teams");
        }
        Err(study_graph::Error::Auth { code, .. }) => println!("  token: blocked — {code}"),
        Err(err) => println!("  token: error — {err}"),
    }
}

fn index_summary(index: &Index) {
    println!("  path: {}", index::default_path().display());
    let now = app::now();
    for kind in [SourceKind::Moodle, SourceKind::Teams] {
        let synced = match index.sources.iter().find(|s| s.kind == kind) {
            Some(source) => fmt::human_age(source.last_sync, now),
            None => "never".to_string(),
        };
        println!("  {} synced: {synced}", kind.as_str());
    }

    let nodes = |kind: NodeKind| index.nodes.iter().filter(|n| n.kind == kind).count();
    let events = |kind: EventKind| index.events.iter().filter(|e| e.kind == kind).count();
    println!("  courses: {}", nodes(NodeKind::Course));
    println!("  teams: {}", nodes(NodeKind::Team));
    println!("  files: {}", index.files.len());
    println!("  assignments: {}", events(EventKind::Coursework));
    println!("  events: {}", events(EventKind::Calendar));
}
