//! `study put <FILE>... --to <COURSE>/<ASSIGNMENT>` — upload a draft
//! submission, optionally submitting it for grading (SPEC §7).

use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use study_core::index::{Event, EventKind, Index, NodeKind};

use crate::path::{self, Target};
use crate::{app, sync};

pub async fn run(files: &[PathBuf], to: &str, submit: bool, yes: bool) -> Result<()> {
    let bad = rejected(files);
    if !bad.is_empty() {
        let listed: Vec<String> = bad.iter().map(|path| path.display().to_string()).collect();
        bail!("not a file: {}", listed.join(", "));
    }
    let Some((course_ref, title_ref)) = to.split_once('/') else {
        bail!("--to takes <course>/<assignment>, got '{to}'");
    };

    let config = app::load_config()?;
    let mut index = app::fresh_index(&config).await?;
    let Target::Node(course) = path::resolve(&index, &config, course_ref)? else {
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

    let client = app::moodle_client(&config)?;
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
