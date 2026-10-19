//! Background work. Every network call runs in a spawned task and reports
//! back through one channel, so the UI loop never blocks.

use std::path::PathBuf;

use study_core::index::{File, Index};

use crate::tui::reader::{self, Reader};
use crate::tui::state::App;
use crate::tui::tree;
use crate::{app, download, path, sync};

pub enum Msg {
    Status(String),
    Downloaded(PathBuf),
    Synced(Index),
    Page { node_id: String, reader: Reader },
    Error(String),
}

/// Applies one message from a finished (or progressing) task.
pub fn apply(app: &mut App, msg: Msg) {
    app.busy = false;
    match msg {
        Msg::Status(text) => {
            app.busy = true;
            app.set_status(text);
        }
        Msg::Downloaded(path) => app.set_status(format!("saved {}", path.display())),
        Msg::Synced(index) => {
            app.replace_index(index);
            app.set_status("synced");
        }
        Msg::Page { node_id, reader } => {
            app.pages.insert(node_id, reader.clone());
            app.reader = Some(reader);
            app.mode = crate::tui::state::Mode::Reader;
            app.set_status("");
        }
        Msg::Error(text) => app.set_error(text),
    }
}

pub fn spawn_download(app: &mut App, file: File) {
    let Some(client) = client(app) else {
        return;
    };
    let dir = app.download_dir.clone();
    let tx = app.tx.clone();
    app.busy = true;
    app.set_status(format!("downloading {}…", file.name));
    tokio::spawn(async move {
        let msg = match download::download(&client, &file, &dir, false, false, true).await {
            Ok(path) => Msg::Downloaded(path),
            Err(err) => Msg::Error(format!("{err:#}")),
        };
        let _ = tx.send(msg);
    });
}

/// Downloads everything under a node into `<download_dir>/<node name>/…`.
pub fn spawn_download_node(app: &mut App, node_id: &str) {
    let Some(node) = app.index.node(node_id) else {
        return;
    };
    let files = tree::files_under(&app.index, node);
    if files.is_empty() {
        app.set_error(format!("nothing to download under {}", node.name));
        return;
    }
    let Some(client) = client(app) else {
        return;
    };
    let dir = app.download_dir.clone();
    let tx = app.tx.clone();
    app.busy = true;
    app.set_status(format!("downloading {} files…", files.len()));
    tokio::spawn(async move {
        let total = files.len();
        for (i, (file, subdir)) in files.iter().enumerate() {
            let status = format!("downloading {}/{total}: {}", i + 1, file.name);
            let _ = tx.send(Msg::Status(status));
            let target = dir.join(subdir);
            if let Err(err) = download::download(&client, file, &target, false, false, true).await {
                let _ = tx.send(Msg::Error(format!("{err:#}")));
                return;
            }
        }
        let _ = tx.send(Msg::Downloaded(dir.join(&files[0].1)));
    });
}

/// Re-crawls Moodle and swaps the index in when done. The config is
/// re-read inside the task because `Config` is not `Clone`.
pub fn spawn_sync(app: &mut App) {
    let tx = app.tx.clone();
    app.busy = true;
    app.set_status("syncing…");
    tokio::spawn(async move {
        let synced = match app::load_config() {
            Ok(config) => sync::sync(&config, true).await,
            Err(err) => Err(err),
        };
        let msg = match synced {
            Ok(index) => Msg::Synced(index),
            Err(err) => Msg::Error(format!("{err:#}")),
        };
        let _ = tx.send(msg);
    });
}

/// Opens the reader on a page module, fetching it unless already cached.
pub fn spawn_page(app: &mut App, node_id: &str) {
    if let Some(reader) = app.pages.get(node_id) {
        apply(
            app,
            Msg::Page {
                node_id: node_id.to_string(),
                reader: reader.clone(),
            },
        );
        return;
    }
    let chain = path::ancestors(&app.index, node_id);
    let (Some(course), Some(node)) = (chain.first(), chain.last()) else {
        return;
    };
    let (Ok(course_id), Ok(cmid)) = (
        course.remote_id.parse::<u64>(),
        node.remote_id.parse::<u64>(),
    ) else {
        app.set_error("not a Moodle page");
        return;
    };
    let web_url = node.web_url.clone();
    let node_id = node_id.to_string();
    let width = reader_width(app.width);
    let Some(client) = client(app) else {
        return;
    };
    let tx = app.tx.clone();
    app.busy = true;
    app.set_status("loading…");
    tokio::spawn(async move {
        let msg = match reader::load_page(&client, course_id, cmid, width).await {
            Ok(mut reader) => {
                reader.web_url = web_url;
                Msg::Page { node_id, reader }
            }
            Err(err) => Msg::Error(format!("{err:#}")),
        };
        let _ = tx.send(msg);
    });
}

/// Text width inside the reader box (`draw.rs` sizes it the same way).
pub fn reader_width(terminal_width: u16) -> usize {
    (usize::from(terminal_width) * 4 / 5)
        .saturating_sub(4)
        .max(20)
}

fn client(app: &mut App) -> Option<study_moodle::MoodleClient> {
    let client = app.client.clone();
    if client.is_none() {
        app.set_error("not logged in — run `study login moodle <url>`");
    }
    client
}
