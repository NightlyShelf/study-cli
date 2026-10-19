//! `study put <FILE>... --to <TARGET>` (SPEC §7): a draft submission to a
//! Moodle assignment (`<course>/<assignment>`, optionally submitted for
//! grading), or files into a Teams channel folder
//! (`<team>[/<channel>][/<folder>...]`).

use std::fs;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use study_core::config::Config;
use study_core::index::{Event, EventKind, Index, Node, NodeKind};
use study_graph::api::SMALL_UPLOAD_LIMIT;

use crate::path::{self, Target};
use crate::{app, sync};

pub async fn run(files: &[PathBuf], to: &str, submit: bool, yes: bool) -> Result<()> {
    let bad = rejected(files);
    if !bad.is_empty() {
        let listed: Vec<String> = bad.iter().map(|path| path.display().to_string()).collect();
        bail!("not a file: {}", listed.join(", "));
    }
    let config = app::load_config()?;
    let index = app::fresh_index(&config).await?;

    // The first segment decides the route: a Team takes the whole reference
    // as a folder path, a Moodle course takes the rest as an assignment name.
    let course_ref = to.split('/').next().unwrap_or_default();
    let Target::Node(course) = path::resolve(&index, &config, course_ref)? else {
        bail!("'{course_ref}' is a file, not a course");
    };
    if course.kind == NodeKind::Team {
        if submit {
            bail!("--submit only applies to Moodle assignments");
        }
        return put_teams(&config, &index, files, to).await;
    }
    put_moodle(&config, index, files, to, submit, yes).await
}

async fn put_moodle(
    config: &Config,
    mut index: Index,
    files: &[PathBuf],
    to: &str,
    submit: bool,
    yes: bool,
) -> Result<()> {
    let Some((course_ref, title_ref)) = to.split_once('/') else {
        bail!("--to takes <course>/<assignment>, got '{to}'");
    };
    let Target::Node(course) = path::resolve(&index, config, course_ref)? else {
        bail!("'{course_ref}' is a file, not a course");
    };
    if course.kind != NodeKind::Course {
        bail!("'{}' is not a course", course.name);
    }
    let event = pick_assignment(&index, &course.id, title_ref)?;
    let assignid = assignment_id(&event.id)?;
    // Owned copies, since the index is edited after the upload.
    let event_id = event.id.clone();
    let title = event.title.clone();

    let client = app::moodle_client(config)?;
    let mut itemid = None;
    for file in files {
        let id = client
            .upload_file(file, itemid)
            .await
            .with_context(|| format!("uploading {}", file.display()))?;
        itemid = Some(id);
    }
    let Some(itemid) = itemid else {
        bail!("no files given");
    };
    client
        .save_submission(assignid, itemid)
        .await
        .context("saving the draft submission")?;
    println!("uploaded {} file(s) as a draft to \"{title}\"", files.len());
    for file in files {
        println!("  {}", file.display());
    }

    if submit {
        confirm_submit(yes)?;
        client
            .submit_for_grading(assignid)
            .await
            .context("submitting for grading")?;
        println!("submitted \"{title}\" for grading");
    }

    // So `todo` shows the new state before the next sync.
    match client.submission_status(assignid).await {
        Ok(status) => {
            if let Some(event) = index.events.iter_mut().find(|e| e.id == event_id) {
                event.status = Some(sync::submission_state(&status));
                event.grade = status.grade;
            }
            app::save_index(&index)?;
        }
        Err(err) => eprintln!("warning: could not refresh the submission status: {err}"),
    }
    Ok(())
}

/// Uploads into the channel folder `to` names; an existing file of the
/// same name is replaced (Graph's default for a named PUT).
async fn put_teams(config: &Config, index: &Index, files: &[PathBuf], to: &str) -> Result<()> {
    let target = path::resolve(index, config, to)?;
    let folder = upload_folder(index, target)?;
    let (drive, parent) = app::split_remote_id(&folder.remote_id)?;
    let location: Vec<&str> = path::ancestors(index, &folder.id)
        .iter()
        .map(|node| node.name.as_str())
        .collect();
    let location = location.join("/");

    let graph = app::graph_client(config)?;
    for file in files {
        let name = file
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        let size = fs::metadata(file)?.len();
        let upload = if size <= SMALL_UPLOAD_LIMIT as u64 {
            graph
                .upload_small(drive, parent, &name, fs::read(file)?)
                .await
        } else {
            graph.upload_large(drive, parent, &name, file).await
        };
        upload.with_context(|| format!("uploading {}", file.display()))?;
        println!("uploaded {name} to {location}");
    }
    app::persist_graph_token(&graph)
}

/// The node files can be uploaded into: a single-channel team (its files
/// folder is the team's remote id), a channel, or a folder. A team with
/// several channels has no folder of its own, so one must be named.
fn upload_folder<'a>(index: &'a Index, target: Target<'a>) -> Result<&'a Node> {
    let node = match target {
        Target::Node(node) => node,
        Target::File(file) => bail!("'{}' is a file — give its folder", file.name),
    };
    match node.kind {
        NodeKind::Channel | NodeKind::Folder => Ok(node),
        NodeKind::Team if node.remote_id.contains('/') => Ok(node),
        NodeKind::Team => {
            let channels: Vec<&str> = index
                .children(&node.id)
                .into_iter()
                .filter(|child| child.kind == NodeKind::Channel)
                .map(|child| child.name.as_str())
                .collect();
            bail!(
                "'{}' has several channels — pick a channel: {}",
                node.name,
                channels.join(", ")
            )
        }
        _ => bail!("'{}' is not a folder", node.name),
    }
}

/// Paths that are not regular files; checked before any network call.
fn rejected(files: &[PathBuf]) -> Vec<&Path> {
    files
        .iter()
        .filter(|path| !path.is_file())
        .map(PathBuf::as_path)
        .collect()
}

/// The one assignment of the course matching `query`; ambiguity lists the
/// candidates and exits 2, like a path reference.
fn pick_assignment<'a>(index: &'a Index, course_id: &str, query: &str) -> Result<&'a Event> {
    let found = assignment_candidates(index, course_id, query);
    if let [event] = found[..] {
        return Ok(event);
    }
    if found.is_empty() {
        bail!("no assignment matches '{query}' — see `study todo --all`");
    }
    eprintln!("'{query}' is ambiguous — did you mean:");
    for event in &found {
        eprintln!("  {}", event.title);
    }
    std::process::exit(2)
}

/// Coursework events of the course whose title matches `query` exactly
/// (case-insensitively), else by prefix.
fn assignment_candidates<'a>(index: &'a Index, course_id: &str, query: &str) -> Vec<&'a Event> {
    let events: Vec<&Event> = index
        .events
        .iter()
        .filter(|event| event.kind == EventKind::Coursework)
        .filter(|event| event.course_id.as_deref() == Some(course_id))
        .collect();
    let titles: Vec<&str> = events.iter().map(|event| event.title.as_str()).collect();
    path::matching(&titles, query)
        .into_iter()
        .map(|i| events[i])
        .collect()
}

/// The assignment instance id out of `moodle:assign:<id>`.
fn assignment_id(event_id: &str) -> Result<u64> {
    let id = event_id
        .strip_prefix("moodle:assign:")
        .and_then(|id| id.parse().ok());
    match id {
        Some(id) => Ok(id),
        None => bail!("'{event_id}' is not a Moodle assignment"),
    }
}

/// Submitting is final on most sites, so it needs `--yes` or a typed `y`.
fn confirm_submit(yes: bool) -> Result<()> {
    eprintln!("warning: submitting for grading is usually irreversible");
    if yes {
        return Ok(());
    }
    if !std::io::stdin().is_terminal() {
        bail!("stdin is not a terminal — pass --yes to submit for grading");
    }
    eprint!("Submit for grading? [y/N] ");
    std::io::stderr().flush()?;
    let mut answer = String::new();
    std::io::stdin().read_line(&mut answer)?;
    if !matches!(answer.trim(), "y" | "Y") {
        eprintln!("aborted — the files stay uploaded as a draft");
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use study_core::index::{Source, SourceKind};

    use super::*;

    fn event(id: &str, course_id: &str, title: &str) -> Event {
        Event {
            id: id.to_string(),
            source: SourceKind::Moodle,
            kind: EventKind::Coursework,
            title: title.to_string(),
            start: None,
            due: None,
            course_id: Some(course_id.to_string()),
            cmid: None,
            status: None,
            grade: None,
            url: None,
        }
    }

    fn sample() -> Index {
        let mut index = Index::default();
        index.replace_source(
            Source {
                kind: SourceKind::Moodle,
                name: "site".to_string(),
                remote_id: "site".to_string(),
                last_sync: Some(1),
                delta_token: None,
            },
            vec![],
            vec![],
            vec![
                event("moodle:assign:1", "moodle:7", "Homework 1"),
                event("moodle:assign:2", "moodle:7", "Homework 10"),
                event("moodle:assign:3", "moodle:7", "Essay"),
                event("moodle:assign:4", "moodle:8", "Homework 1"),
            ],
        );
        index
    }

    #[test]
    fn exact_title_wins_over_prefix_within_the_course() {
        let index = sample();
        let found = assignment_candidates(&index, "moodle:7", "homework 1");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "moodle:assign:1");

        let found = assignment_candidates(&index, "moodle:7", "ess");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "moodle:assign:3");

        let found = assignment_candidates(&index, "moodle:8", "homework");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "moodle:assign:4");
    }

    #[test]
    fn prefix_ambiguity_and_misses() {
        let index = sample();
        assert_eq!(
            assignment_candidates(&index, "moodle:7", "homework").len(),
            2
        );
        assert!(assignment_candidates(&index, "moodle:7", "quiz").is_empty());
        assert!(pick_assignment(&index, "moodle:7", "quiz").is_err());
        assert_eq!(
            pick_assignment(&index, "moodle:7", "homework 10")
                .unwrap()
                .id,
            "moodle:assign:2"
        );
    }

    #[test]
    fn parses_assignment_ids() {
        assert_eq!(assignment_id("moodle:assign:55").unwrap(), 55);
        assert!(assignment_id("moodle:mod:55").is_err());
        assert!(assignment_id("moodle:assign:x").is_err());
    }

    fn teams_node(
        id: &str,
        kind: NodeKind,
        parent: Option<&str>,
        name: &str,
        remote: &str,
    ) -> Node {
        Node {
            id: id.to_string(),
            source: SourceKind::Teams,
            parent: parent.map(str::to_string),
            kind,
            modname: (kind == NodeKind::Module).then(|| "messages".to_string()),
            name: name.to_string(),
            remote_id: remote.to_string(),
            web_url: None,
        }
    }

    /// One flattened team and one with two channels, as `teams_sync` writes
    /// them.
    fn teams_sample() -> Index {
        let mut index = Index::default();
        index.replace_source(
            Source {
                kind: SourceKind::Teams,
                name: "Microsoft Teams".to_string(),
                remote_id: "teams".to_string(),
                last_sync: Some(1),
                delta_token: None,
            },
            vec![
                teams_node("teams:t1", NodeKind::Team, None, "Algorithms", "b!d/root1"),
                teams_node(
                    "teams:msg:c1",
                    NodeKind::Module,
                    Some("teams:t1"),
                    "Messages (General)",
                    "t1/c1",
                ),
                teams_node(
                    "teams:item:f1",
                    NodeKind::Folder,
                    Some("teams:t1"),
                    "Week 1",
                    "b!d/f1",
                ),
                teams_node("teams:t2", NodeKind::Team, None, "Databases", "t2"),
                teams_node(
                    "teams:chan:c2",
                    NodeKind::Channel,
                    Some("teams:t2"),
                    "General",
                    "b!e/root2",
                ),
                teams_node(
                    "teams:chan:c3",
                    NodeKind::Channel,
                    Some("teams:t2"),
                    "Labs",
                    "b!e/root3",
                ),
            ],
            vec![],
            vec![],
        );
        index
    }

    #[test]
    fn flattened_team_channel_and_folder_take_uploads() {
        let index = teams_sample();
        let config = Config::default();
        for (reference, expected) in [
            ("algorithms", "teams:t1"),
            ("algorithms/week 1", "teams:item:f1"),
            ("databases/labs", "teams:chan:c3"),
        ] {
            let target = path::resolve(&index, &config, reference).unwrap();
            assert_eq!(upload_folder(&index, target).unwrap().id, expected);
        }
    }

    #[test]
    fn multi_channel_team_and_messages_are_refused() {
        let index = teams_sample();
        let config = Config::default();

        let target = path::resolve(&index, &config, "databases").unwrap();
        let err = upload_folder(&index, target).unwrap_err().to_string();
        assert!(err.contains("pick a channel: General, Labs"), "{err}");

        let target = path::resolve(&index, &config, "algorithms/messages").unwrap();
        assert!(upload_folder(&index, target).is_err());
    }

    #[test]
    fn rejects_directories_and_missing_paths() {
        let dir = tempfile::tempdir().unwrap();
        let ok = dir.path().join("hw.pdf");
        fs::write(&ok, b"pdf").unwrap();
        let missing = dir.path().join("missing.pdf");
        let files = vec![ok.clone(), dir.path().to_path_buf(), missing.clone()];

        let bad = rejected(&files);
        assert_eq!(bad, [dir.path(), missing.as_path()]);
        assert!(rejected(&[ok]).is_empty());
    }
}
