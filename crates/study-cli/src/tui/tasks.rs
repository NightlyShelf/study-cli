//! Background work. Every network call runs in a spawned task and reports
//! back through one channel, so the UI loop never blocks.

use std::path::{Path, PathBuf};

use study_core::index::{File, Index};
use study_graph::GraphClient;
use study_moodle::MoodleClient;
use tokio::sync::mpsc::UnboundedSender;

use crate::tui::reader::{self, Reader};
use crate::tui::state::App;
use crate::tui::tree;
use crate::{app, download, path, sync};

pub enum Msg {
    Status(String),
    Downloaded(PathBuf),
    Synced(Index),
    Page {
        node_id: String,
        reader: Reader,
    },
    /// A submission draft now holds `file_name`.
    Uploaded {
        event_id: String,
        file_name: String,
    },
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
        Msg::Uploaded {
            event_id,
            file_name,
        } => mark_uploaded(app, &event_id, &file_name),
        Msg::Error(text) => app.set_error(text),
    }
}

/// Records the draft locally so the Todo tab is right before the next
/// sync, and persists it for the CLI.
fn mark_uploaded(app: &mut App, event_id: &str, file_name: &str) {
    let Some(event) = app
        .index
        .events
        .iter_mut()
        .find(|event| event.id == event_id)
    else {
        app.set_status(format!("uploaded {file_name} as a draft"));
        return;
    };
    event.status = Some("draft".to_string());
    let title = event.title.clone();
    app.rebuild_todo();
    match app::save_index(&app.index) {
        Ok(()) => app.set_status(format!("uploaded {file_name} as a draft to \"{title}\"")),
        Err(err) => app.set_error(format!("{err:#}")),
    }
}

/// Both clients go along; `app::fetch` picks one by the file's source and
/// says which login is missing.
pub fn spawn_download(app: &mut App, file: File) {
    let (moodle, graph) = (app.client.clone(), app.graph.clone());
    let dir = app.download_dir.clone();
    let tx = app.tx.clone();
    app.busy = true;
    app.set_status(format!("downloading {}…", file.name));
    tokio::spawn(async move {
        let msg = match fetch_and_save(&moodle, &graph, &file, &dir).await {
            Ok(path) => Msg::Downloaded(path),
            Err(err) => Msg::Error(format!("{err:#}")),
        };
        let _ = tx.send(msg);
        if is_teams(&file) {
            persist_graph_token(graph.as_ref(), &tx);
        }
    });
}

async fn fetch_and_save(
    moodle: &Option<MoodleClient>,
    graph: &Option<GraphClient>,
    file: &File,
    dir: &Path,
) -> anyhow::Result<PathBuf> {
    let response = app::fetch(moodle.as_ref(), graph.as_ref(), file).await?;
    download::download(response, file, dir, false, false, true).await
}

fn is_teams(file: &File) -> bool {
    file.id.starts_with("teams:")
}

/// Entra rotates the refresh token on every refresh, so it is written back
/// after each Graph use. The work itself succeeded, so a failure here is
/// only reported: at worst the next start needs `study login teams`.
fn persist_graph_token(graph: Option<&GraphClient>, tx: &UnboundedSender<Msg>) {
    let Some(graph) = graph else {
        return;
    };
    if let Err(err) = app::persist_graph_token(graph) {
        let _ = tx.send(Msg::Error(format!("teams token not saved: {err:#}")));
    }
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
    let (moodle, graph) = (app.client.clone(), app.graph.clone());
    let dir = app.download_dir.clone();
    let tx = app.tx.clone();
    app.busy = true;
    app.set_status(format!("downloading {} files…", files.len()));
    tokio::spawn(async move {
        let msg = match save_all(&moodle, &graph, &files, &dir, &tx).await {
            Ok(()) => Msg::Downloaded(dir.join(&files[0].1)),
            Err(err) => Msg::Error(format!("{err:#}")),
        };
        let _ = tx.send(msg);
        if is_teams(&files[0].0) {
            persist_graph_token(graph.as_ref(), &tx);
        }
    });
}

/// Downloads `files` one by one into their subdirectories of `dir`,
/// reporting progress on `tx`; the first failure stops the batch.
async fn save_all(
    moodle: &Option<MoodleClient>,
    graph: &Option<GraphClient>,
    files: &[(File, PathBuf)],
    dir: &Path,
    tx: &UnboundedSender<Msg>,
) -> anyhow::Result<()> {
    let total = files.len();
    for (i, (file, subdir)) in files.iter().enumerate() {
        let status = format!("downloading {}/{total}: {}", i + 1, file.name);
        let _ = tx.send(Msg::Status(status));
        fetch_and_save(moodle, graph, file, &dir.join(subdir)).await?;
    }
    Ok(())
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

/// Reopens a reader fetched earlier this session. True when it did.
fn open_cached(app: &mut App, node_id: &str) -> bool {
    let Some(reader) = app.pages.get(node_id) else {
        return false;
    };
    let msg = Msg::Page {
        node_id: node_id.to_string(),
        reader: reader.clone(),
    };
    apply(app, msg);
    true
}

/// Opens the reader on a page module, fetching it unless already cached.
pub fn spawn_page(app: &mut App, node_id: &str) {
    if open_cached(app, node_id) {
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

/// Opens the reader on a channel's transcript (SPEC §4.3), fetching it
/// unless already cached. The entry's `remote_id` is `<team>/<channel>`;
/// its parent, the team or channel, names the reader.
pub fn spawn_messages(app: &mut App, node_id: &str) {
    if open_cached(app, node_id) {
        return;
    }
    let Some(node) = app.index.node(node_id) else {
        return;
    };
    let Some((team_id, channel_id)) = node.remote_id.split_once('/') else {
        app.set_error("not a Teams channel");
        return;
    };
    let (team_id, channel_id) = (team_id.to_string(), channel_id.to_string());
    let title = node
        .parent
        .as_deref()
        .and_then(|parent| app.index.node(parent))
        .map_or(node.name.clone(), |parent| parent.name.clone());
    let web_url = node.web_url.clone();
    let node_id = node_id.to_string();
    let width = reader_width(app.width);
    let Some(graph) = app.graph.clone() else {
        app.set_error("not logged in to Teams — run `study login teams`");
        return;
    };
    let tx = app.tx.clone();
    app.busy = true;
    app.set_status("loading…");
    tokio::spawn(async move {
        let msg = match reader::load_messages(&graph, &team_id, &channel_id, width).await {
            Ok(mut reader) => {
                reader.title = title;
                reader.web_url = web_url;
                Msg::Page { node_id, reader }
            }
            Err(err) => Msg::Error(format!("{err:#}")),
        };
        let _ = tx.send(msg);
        persist_graph_token(Some(&graph), &tx);
    });
}

/// Puts `path` into a fresh draft area and attaches it to the assignment's
/// file submission. A draft only: submitting for grading is irreversible,
/// so it stays a deliberate CLI flag.
pub fn spawn_upload(app: &mut App, assignid: u64, path: PathBuf) {
    let Some(client) = client(app) else {
        return;
    };
    let tx = app.tx.clone();
    app.busy = true;
    app.set_status(format!("uploading {}…", path.display()));
    tokio::spawn(async move {
        let file_name = path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let saved = match client.upload_file(&path, None).await {
            Ok(itemid) => client.save_submission(assignid, itemid).await,
            Err(err) => Err(err),
        };
        let msg = match saved {
            Ok(()) => Msg::Uploaded {
                event_id: format!("moodle:assign:{assignid}"),
                file_name,
            },
            Err(err) => Msg::Error(format!("upload failed: {err}")),
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
