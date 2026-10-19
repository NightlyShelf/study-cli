//! `study sync` — crawl Moodle (here) and Teams (`teams_sync`) into the
//! index (SPEC §6).

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use indicatif::ProgressBar;
use study_core::config::Config;
use study_core::index::{Event, EventKind, File, Index, Node, NodeKind, Source, SourceKind};
use study_moodle::MoodleClient;
use study_moodle::api::{
    Assignment, Attachment, CalendarEvent, Content, Course, Module, Section, SubmissionStatus,
};
use tokio::task::JoinSet;

use crate::{app, teams_sync};

/// Submission status is fetched per assignment; past this age the user no
/// longer cares, and skipping it keeps sync fast on long-running sites.
pub const TODO_PAST_SECS: u64 = 14 * 86_400;

const STATUS_IN_FLIGHT: usize = 8;

/// The calendar window: last week, so a just-missed event still shows, and
/// the next three months, which covers a semester's exam dates.
const CALENDAR_PAST_SECS: u64 = 7 * 86_400;
const CALENDAR_FUTURE_SECS: u64 = 90 * 86_400;

pub async fn run() -> Result<()> {
    let config = app::load_config()?;
    sync(&config, false).await?;
    Ok(())
}

/// Refreshes every source the user is logged in to; one that is not is
/// skipped with a note. `quiet` silences the spinners and summary, which
/// would corrupt the TUI's alternate screen.
pub async fn sync(config: &Config, quiet: bool) -> Result<Index> {
    let moodle = app::moodle_client(config);
    let graph = app::graph_client(config);
    if let (Err(moodle_err), Err(graph_err)) = (&moodle, &graph) {
        bail!("{moodle_err}\n{graph_err}");
    }

    let mut index = app::load_index()?;
    let mut summary = Vec::new();
    match moodle {
        Ok(client) => summary.push(sync_moodle(&client, &mut index, quiet).await?),
        Err(err) if !quiet => eprintln!("skipping Moodle: {err}"),
        Err(_) => {}
    }
    match graph {
        Ok(client) => summary.push(teams_sync::sync(&client, &mut index, quiet).await?),
        Err(err) if !quiet => eprintln!("skipping Teams: {err}"),
        Err(_) => {}
    }
    if !quiet {
        eprintln!("synced {}", summary.join(", "));
    }
    app::save_index(&index)?;
    Ok(index)
}

/// Full Moodle crawl into `index`: site info → courses → contents of each
/// course. ~10 round-trips, so a sequential loop is fine. Returns the
/// summary fragment for the sync line.
async fn sync_moodle(client: &MoodleClient, index: &mut Index, quiet: bool) -> Result<String> {
    let site = match client.site_info().await {
        Ok(site) => site,
        Err(study_moodle::Error::TokenExpired(_)) => {
            bail!("Moodle token expired — run `study login moodle`")
        }
        Err(err) => return Err(err).context("fetching site info"),
    };
    let courses = client
        .user_courses(site.userid)
        .await
        .context("listing courses")?;

    let spinner = if quiet {
        ProgressBar::hidden()
    } else {
        ProgressBar::new_spinner()
    };
    spinner.enable_steady_tick(Duration::from_millis(100));
    let mut nodes = Vec::new();
    let mut files = Vec::new();
    for course in &courses {
        spinner.set_message(course.fullname.clone());
        let sections = client
            .course_contents(course.id)
            .await
            .with_context(|| format!("fetching contents of '{}'", course.fullname))?;
        let (course_nodes, course_files) = map_course(&client.base_url, course, &sections);
        nodes.extend(course_nodes);
        files.extend(course_files);
    }

    spinner.set_message("assignments");
    let course_ids: Vec<u64> = courses.iter().map(|course| course.id).collect();
    let assignments = client
        .assignments(&course_ids)
        .await
        .context("listing assignments")?;
    for assignment in &assignments {
        files.extend(map_handouts(assignment, &nodes));
    }
    let statuses = submission_statuses(client, &assignments, app::now(), quiet).await;
    let mut events: Vec<Event> = assignments
        .iter()
        .map(|assignment| {
            map_assignment(&client.base_url, assignment, statuses.get(&assignment.id))
        })
        .collect();

    spinner.set_message("calendar");
    let calendar = calendar_events(client, &course_ids, app::now(), quiet).await;
    let calendar_count = calendar.len();
    events.extend(calendar);
    spinner.finish_and_clear();
    let summary = format!(
        "{} courses, {} files, {} assignments, {} events",
        courses.len(),
        files.len(),
        assignments.len(),
        calendar_count
    );

    let source = Source {
        kind: SourceKind::Moodle,
        name: site.sitename,
        remote_id: client.base_url.clone(),
        last_sync: Some(app::now()),
        delta_token: None,
    };
    index.replace_source(source, nodes, files, events);
    Ok(summary)
}

/// Visible calendar events around now, as index rows. A site can have its
/// calendar disabled, so a failed call only loses the events, never the sync.
async fn calendar_events(
    client: &MoodleClient,
    course_ids: &[u64],
    now: u64,
    quiet: bool,
) -> Vec<Event> {
    let from = now.saturating_sub(CALENDAR_PAST_SECS);
    let result = client
        .calendar_events(course_ids, from, now + CALENDAR_FUTURE_SECS)
        .await;
    let raw = match result {
        Ok(raw) => raw,
        Err(err) => {
            if !quiet {
                eprintln!("warning: no calendar events: {err}");
            }
            return Vec::new();
        }
    };
    raw.iter()
        .filter(|event| event.visible != 0)
        .map(|event| map_calendar_event(&client.base_url, event))
        .collect()
}

/// The index row for one calendar event. A `due`-type event for an
/// assignment stays next to the assignment's own coursework row: it is
/// still a calendar entry.
pub fn map_calendar_event(base_url: &str, event: &CalendarEvent) -> Event {
    Event {
        id: format!("moodle:cal:{}", event.id),
        source: SourceKind::Moodle,
        kind: EventKind::Calendar,
        title: event.name.clone(),
        start: Some(event.timestart),
        due: (event.timeduration > 0).then(|| event.timestart + event.timeduration),
        course_id: (event.courseid > 0).then(|| format!("moodle:{}", event.courseid)),
        cmid: None,
        // `status` carries Moodle's `eventtype` (`due`, `open`, `close`,
        // `course`, `user`…): the struct has no better field and the
        // calendar views show it.
        status: Some(event.eventtype.clone()),
        grade: None,
        url: Some(format!(
            "{base_url}/calendar/view.php?view=day&time={}",
            event.timestart
        )),
    }
}

/// One `mod_assign_get_submission_status` call per assignment that is not
/// long past, at most [`STATUS_IN_FLIGHT`] at a time. A failed call only
/// loses that assignment's status, never the sync.
async fn submission_statuses(
    client: &MoodleClient,
    assignments: &[Assignment],
    now: u64,
    quiet: bool,
) -> HashMap<u64, SubmissionStatus> {
    let mut pending = JoinSet::new();
    let mut statuses = HashMap::new();
    for assignment in assignments {
        if !is_current(assignment.duedate, now) {
            continue;
        }
        if pending.len() >= STATUS_IN_FLIGHT
            && let Some(joined) = pending.join_next().await
        {
            record_status(joined, &mut statuses, quiet);
        }
        let client = client.clone();
        let id = assignment.id;
        let name = assignment.name.clone();
        pending.spawn(async move { (id, name, client.submission_status(id).await) });
    }
    while let Some(joined) = pending.join_next().await {
        record_status(joined, &mut statuses, quiet);
    }
    statuses
}

type StatusResult = (u64, String, study_moodle::Result<SubmissionStatus>);

fn record_status(
    joined: std::result::Result<StatusResult, tokio::task::JoinError>,
    statuses: &mut HashMap<u64, SubmissionStatus>,
    quiet: bool,
) {
    // A join error means the task panicked; nothing to record.
    let Ok((id, name, result)) = joined else {
        return;
    };
    match result {
        Ok(status) => {
            statuses.insert(id, status);
        }
        Err(err) if !quiet => eprintln!("warning: no submission status for '{name}': {err}"),
        Err(_) => {}
    }
}

/// True unless the due date is more than [`TODO_PAST_SECS`] ago; an
/// assignment without a due date is always current.
pub fn is_current(due: Option<u64>, now: u64) -> bool {
    match due {
        Some(due) => due + TODO_PAST_SECS >= now,
        None => true,
    }
}

/// The index row for one assignment. `status` is `None` when its submission
/// status was not fetched (too old, or the call failed).
pub fn map_assignment(
    base_url: &str,
    assignment: &Assignment,
    status: Option<&SubmissionStatus>,
) -> Event {
    Event {
        id: format!("moodle:assign:{}", assignment.id),
        source: SourceKind::Moodle,
        kind: EventKind::Coursework,
        title: assignment.name.clone(),
        // Moodle stores "no date" as 0 (the api already clears `duedate`).
        start: assignment.allowsubmissionsfromdate.filter(|&at| at != 0),
        due: assignment.duedate.filter(|&at| at != 0),
        course_id: Some(format!("moodle:{}", assignment.course)),
        cmid: Some(assignment.cmid.to_string()),
        status: status.map(submission_state),
        grade: status.and_then(|status| status.grade.clone()),
        url: Some(format!(
            "{base_url}/mod/assign/view.php?id={}",
            assignment.cmid
        )),
    }
}

/// `new` | `draft` | `submitted` | `graded`, as `todo` shows it.
pub fn submission_state(status: &SubmissionStatus) -> String {
    if status.grade.is_some() {
        return "graded".to_string();
    }
    status.status.clone().unwrap_or_else(|| "new".to_string())
}

/// Handouts go on the assignment's module node so they show up in `find`
/// and `get`. A hidden module has no node, and then its handouts stay out.
pub fn map_handouts(assignment: &Assignment, nodes: &[Node]) -> Vec<File> {
    let node_id = format!("moodle:mod:{}", assignment.cmid);
    if !nodes.iter().any(|node| node.id == node_id) {
        return Vec::new();
    }
    assignment
        .introattachments
        .iter()
        .filter_map(|attachment| map_attachment(assignment.cmid, &node_id, attachment))
        .collect()
}

/// A handout has the same fields as a module content minus `type` and
/// `sortorder`, so it takes the same route as every other file.
fn map_attachment(cmid: u64, node_id: &str, attachment: &Attachment) -> Option<File> {
    let content = Content {
        kind: "file".to_string(),
        filename: attachment.filename.clone(),
        filepath: attachment.filepath.clone(),
        filesize: attachment.filesize,
        fileurl: attachment.fileurl.clone(),
        mimetype: attachment.mimetype.clone(),
        timemodified: attachment.timemodified,
        sortorder: None,
    };
    map_file(cmid, node_id, &content)
}

/// Pure mapping from one course's API payload to index rows. Node ids never
/// contain the token; file urls are stored raw and tokenised at download time.
pub fn map_course(base_url: &str, course: &Course, sections: &[Section]) -> (Vec<Node>, Vec<File>) {
    let course_id = format!("moodle:{}", course.id);
    let mut nodes = vec![Node {
        id: course_id.clone(),
        source: SourceKind::Moodle,
        parent: None,
        kind: NodeKind::Course,
        modname: None,
        name: course.fullname.clone(),
        remote_id: course.id.to_string(),
        web_url: Some(format!("{base_url}/course/view.php?id={}", course.id)),
    }];
    let mut files = Vec::new();

    for section in sections {
        if section.uservisible == Some(false) {
            continue;
        }
        let section_id = format!("moodle:sec:{}", section.id);
        let nodes_before = nodes.len();
        let files_before = files.len();
        for module in &section.modules {
            if module.uservisible == Some(false) {
                continue;
            }
            map_module(&section_id, module, &mut nodes, &mut files);
        }
        // An empty section is just noise in `ls`.
        if nodes.len() == nodes_before && files.len() == files_before {
            continue;
        }
        let section_node = Node {
            id: section_id,
            source: SourceKind::Moodle,
            parent: Some(course_id.clone()),
            kind: NodeKind::Section,
            modname: None,
            name: section.name.clone(),
            remote_id: section.id.to_string(),
            web_url: None,
        };
        nodes.insert(nodes_before, section_node);
    }
    // The same lecture PDF is listed under every page that embeds it (8–9
    // times on the real site); the first copy is enough.
    let mut seen = HashSet::new();
    files.retain(|file| seen.insert((file.name.clone(), file.size, file.mtime)));
    (nodes, files)
}

fn map_module(section_id: &str, module: &Module, nodes: &mut Vec<Node>, files: &mut Vec<File>) {
    // A resource is one file; showing it as its own node would only add a level.
    if module.modname == "resource" {
        files.extend(resource_file(module, section_id));
        return;
    }

    let module_id = format!("moodle:mod:{}", module.id);
    let kind = if module.modname == "folder" {
        NodeKind::Folder
    } else {
        NodeKind::Module
    };
    let web_url = match (module.modname.as_str(), module.contents.first()) {
        ("url", Some(link)) if link.fileurl.is_some() => link.fileurl.clone(),
        _ => module.url.clone(),
    };
    nodes.push(Node {
        id: module_id.clone(),
        source: SourceKind::Moodle,
        parent: Some(section_id.to_string()),
        kind,
        modname: Some(module.modname.clone()),
        name: module.name.clone(),
        remote_id: module.id.to_string(),
        web_url,
    });
    files.extend(module_files(module, &module_id));
}

/// A resource is one document, but an HTML one also lists its support files
/// (`.js`, `.css`, images); Moodle marks the document with `sortorder: 1`.
fn resource_file(module: &Module, node_id: &str) -> Option<File> {
    let main = module.contents.iter().find(|c| c.sortorder == Some(1));
    let content = main.or_else(|| module.contents.iter().find(|c| c.kind == "file"))?;
    map_file(module.id, node_id, content)
}

/// A folder holds whatever was uploaded; any other module (page, assign,
/// forum…) also lists the html/css/js/images its own page is built from.
fn module_files(module: &Module, node_id: &str) -> Vec<File> {
    module
        .contents
        .iter()
        .filter(|content| content.kind == "file")
        // A page's own rendering is an `index.html` with no mimetype; like a
        // resource's main file it is the one content marked `sortorder: 1`.
        .filter(|content| module.modname != "page" || content.sortorder != Some(1))
        .filter(|content| module.modname == "folder" || is_attachment(content.mimetype.as_deref()))
        .filter_map(|content| map_file(module.id, node_id, content))
        .collect()
}

/// True for anything a student would download; false for the files a
/// module's page is made of. An unknown mimetype is kept.
fn is_attachment(mimetype: Option<&str>) -> bool {
    let Some(mime) = mimetype else {
        return true;
    };
    !(mime.starts_with("text/html")
        || mime.starts_with("text/css")
        || mime.starts_with("image/")
        || mime.contains("javascript"))
}

fn map_file(cmid: u64, node_id: &str, content: &Content) -> Option<File> {
    let fileurl = content.fileurl.clone()?;
    let filepath = content.filepath.as_deref().unwrap_or("/");
    // Subfolders stay flat: `sub/notes.pdf` is one file named with its path.
    let name = format!("{filepath}{}", content.filename)
        .trim_start_matches('/')
        .to_string();
    Some(File {
        id: format!("moodle:file:{cmid}:{filepath}{}", content.filename),
        node: node_id.to_string(),
        name,
        size: content.filesize,
        mtime: content.timemodified,
        mime: content.mimetype.clone(),
        remote_id: fileurl.clone(),
        download_url: fileurl,
        web_url: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content(filename: &str, filepath: &str) -> Content {
        Content {
            kind: "file".to_string(),
            filename: filename.to_string(),
            filepath: Some(filepath.to_string()),
            filesize: Some(100),
            fileurl: Some(format!(
                "https://moodle.test/webservice/pluginfile.php/1/{filename}?forcedownload=1"
            )),
            mimetype: Some("application/pdf".to_string()),
            timemodified: Some(1_700_000_000),
            sortorder: None,
        }
    }

    fn ordered(filename: &str, sortorder: u64) -> Content {
        Content {
            sortorder: Some(sortorder),
            ..content(filename, "/")
        }
    }

    fn typed(filename: &str, mimetype: &str) -> Content {
        Content {
            mimetype: Some(mimetype.to_string()),
            ..content(filename, "/")
        }
    }

    fn module(id: u64, name: &str, modname: &str, contents: Vec<Content>) -> Module {
        Module {
            id,
            name: name.to_string(),
            modname: modname.to_string(),
            url: Some(format!(
                "https://moodle.test/mod/{modname}/view.php?id={id}"
            )),
            uservisible: Some(true),
            contents,
            contentsinfo: None,
        }
    }

    fn fixture() -> (Course, Vec<Section>) {
        let course = Course {
            id: 7,
            shortname: "ALG".to_string(),
            fullname: "Algorithms".to_string(),
            lastaccess: None,
        };
        let mut link = Content {
            kind: "url".to_string(),
            ..content("Course site", "/")
        };
        link.fileurl = Some("https://example.org".to_string());
        let sections = vec![
            Section {
                id: 10,
                name: "Week 1".to_string(),
                uservisible: Some(true),
                modules: vec![
                    module(101, "Slides", "resource", vec![content("slides.pdf", "/")]),
                    module(
                        102,
                        "Exercises",
                        "folder",
                        vec![content("ex1.pdf", "/"), content("notes.pdf", "/sub/")],
                    ),
                    module(103, "Syllabus", "page", vec![]),
                    module(104, "Course site", "url", vec![link]),
                ],
            },
            Section {
                id: 11,
                name: "Hidden".to_string(),
                uservisible: Some(false),
                modules: vec![module(
                    105,
                    "Secret",
                    "resource",
                    vec![content("secret.pdf", "/")],
                )],
            },
            Section {
                id: 12,
                name: "Empty".to_string(),
                uservisible: Some(true),
                modules: vec![],
            },
        ];
        (course, sections)
    }

    #[test]
    fn maps_nodes() {
        let (course, sections) = fixture();
        let (nodes, _) = map_course("https://moodle.test", &course, &sections);

        let ids: Vec<&str> = nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "moodle:7",
                "moodle:sec:10",
                "moodle:mod:102",
                "moodle:mod:103",
                "moodle:mod:104"
            ]
        );
        assert_eq!(nodes[0].kind, NodeKind::Course);
        assert_eq!(nodes[0].name, "Algorithms");
        assert_eq!(
            nodes[0].web_url.as_deref(),
            Some("https://moodle.test/course/view.php?id=7")
        );
        assert_eq!(nodes[1].kind, NodeKind::Section);
        assert_eq!(nodes[1].parent.as_deref(), Some("moodle:7"));
        assert_eq!(nodes[2].kind, NodeKind::Folder);
        assert_eq!(nodes[2].parent.as_deref(), Some("moodle:sec:10"));
        assert_eq!(nodes[2].modname.as_deref(), Some("folder"));
        assert_eq!(nodes[3].kind, NodeKind::Module);
        assert_eq!(nodes[3].modname.as_deref(), Some("page"));
        // A url module points at the link itself, not its Moodle view page.
        assert_eq!(nodes[4].web_url.as_deref(), Some("https://example.org"));
    }

    #[test]
    fn maps_files() {
        let (course, sections) = fixture();
        let (_, files) = map_course("https://moodle.test", &course, &sections);

        let names: Vec<(&str, &str)> = files
            .iter()
            .map(|f| (f.name.as_str(), f.node.as_str()))
            .collect();
        assert_eq!(
            names,
            [
                ("slides.pdf", "moodle:sec:10"),
                ("ex1.pdf", "moodle:mod:102"),
                ("sub/notes.pdf", "moodle:mod:102"),
            ]
        );
        assert_eq!(files[0].id, "moodle:file:101:/slides.pdf");
        assert_eq!(files[2].id, "moodle:file:102:/sub/notes.pdf");
        assert_eq!(files[0].size, Some(100));
        assert_eq!(files[0].mtime, Some(1_700_000_000));
        assert_eq!(files[0].mime.as_deref(), Some("application/pdf"));
        assert_eq!(
            files[0].download_url,
            "https://moodle.test/webservice/pluginfile.php/1/slides.pdf?forcedownload=1"
        );
        assert!(!files.iter().any(|f| f.name == "secret.pdf"));
    }

    #[test]
    fn resource_keeps_only_its_main_file() {
        let (course, _) = fixture();
        let sections = vec![Section {
            id: 20,
            name: "Pages".to_string(),
            uservisible: Some(true),
            modules: vec![
                module(
                    201,
                    "Lecture page",
                    "resource",
                    vec![
                        ordered("tocbot.js", 0),
                        ordered("index.html", 1),
                        ordered("markdown.css", 0),
                    ],
                ),
                module(
                    202,
                    "Old slides",
                    "resource",
                    vec![content("slides.pdf", "/"), content("extra.pdf", "/")],
                ),
            ],
        }];
        let (_, files) = map_course("https://moodle.test", &course, &sections);

        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["index.html", "slides.pdf"]);
    }

    #[test]
    fn page_keeps_attachments_once() {
        let (course, _) = fixture();
        let page_files = || {
            vec![
                Content {
                    sortorder: Some(1),
                    mimetype: None,
                    filesize: Some(0),
                    ..content("index.html", "/")
                },
                typed("markdown.css", "text/css"),
                typed("highlight.js", "application/javascript"),
                typed("diagram.png", "image/png"),
                typed("lecture5.pdf", "application/pdf"),
            ]
        };
        let sections = vec![Section {
            id: 30,
            name: "Lectures".to_string(),
            uservisible: Some(true),
            modules: vec![
                module(301, "Lecture 5", "page", page_files()),
                module(302, "Lecture 5 (video)", "page", page_files()),
            ],
        }];
        let (_, files) = map_course("https://moodle.test", &course, &sections);

        let names: Vec<(&str, &str)> = files
            .iter()
            .map(|f| (f.name.as_str(), f.node.as_str()))
            .collect();
        assert_eq!(names, [("lecture5.pdf", "moodle:mod:301")]);
    }

    fn assignment(id: u64, cmid: u64, duedate: Option<u64>) -> Assignment {
        Assignment {
            id,
            cmid,
            course: 7,
            name: format!("Homework {id}"),
            duedate,
            allowsubmissionsfromdate: Some(0),
            intro: String::new(),
            introattachments: vec![Attachment {
                filename: "hw.pdf".to_string(),
                filepath: Some("/".to_string()),
                filesize: Some(1234),
                fileurl: Some("https://moodle.test/webservice/pluginfile.php/1/hw.pdf".to_string()),
                mimetype: Some("application/pdf".to_string()),
                timemodified: Some(1_699_000_000),
            }],
        }
    }

    #[test]
    fn maps_assignment_without_status() {
        let mut assignment = assignment(55, 9912, Some(1_700_000_000));
        assignment.allowsubmissionsfromdate = Some(1_690_000_000);
        let event = map_assignment("https://moodle.test", &assignment, None);

        assert_eq!(event.id, "moodle:assign:55");
        assert_eq!(event.source, SourceKind::Moodle);
        assert_eq!(event.kind, EventKind::Coursework);
        assert_eq!(event.title, "Homework 55");
        assert_eq!(event.start, Some(1_690_000_000));
        assert_eq!(event.due, Some(1_700_000_000));
        assert_eq!(event.course_id.as_deref(), Some("moodle:7"));
        assert_eq!(event.cmid.as_deref(), Some("9912"));
        assert_eq!(event.status, None);
        assert_eq!(event.grade, None);
        assert_eq!(
            event.url.as_deref(),
            Some("https://moodle.test/mod/assign/view.php?id=9912")
        );
    }

    #[test]
    fn maps_assignment_status_and_zero_dates() {
        let assignment = assignment(56, 9913, None);
        let draft = SubmissionStatus {
            status: Some("draft".to_string()),
            ..SubmissionStatus::default()
        };
        let event = map_assignment("https://moodle.test", &assignment, Some(&draft));
        assert_eq!(event.start, None);
        assert_eq!(event.due, None);
        assert_eq!(event.status.as_deref(), Some("draft"));

        let graded = SubmissionStatus {
            status: Some("submitted".to_string()),
            submitted_at: Some(1),
            grade: Some("85.00 / 100.00".to_string()),
        };
        let event = map_assignment("https://moodle.test", &assignment, Some(&graded));
        assert_eq!(event.status.as_deref(), Some("graded"));
        assert_eq!(event.grade.as_deref(), Some("85.00 / 100.00"));

        let untouched = SubmissionStatus::default();
        let event = map_assignment("https://moodle.test", &assignment, Some(&untouched));
        assert_eq!(event.status.as_deref(), Some("new"));
    }

    #[test]
    fn handouts_attach_to_the_module_node() {
        let (course, sections) = fixture();
        let (mut nodes, _) = map_course("https://moodle.test", &course, &sections);
        let assignment = assignment(55, 9912, None);
        assert!(map_handouts(&assignment, &nodes).is_empty());

        nodes.push(Node {
            id: "moodle:mod:9912".to_string(),
            source: SourceKind::Moodle,
            parent: Some("moodle:sec:10".to_string()),
            kind: NodeKind::Module,
            modname: Some("assign".to_string()),
            name: "Homework 55".to_string(),
            remote_id: "9912".to_string(),
            web_url: None,
        });
        let files = map_handouts(&assignment, &nodes);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].id, "moodle:file:9912:/hw.pdf");
        assert_eq!(files[0].node, "moodle:mod:9912");
        assert_eq!(files[0].name, "hw.pdf");
        assert_eq!(files[0].size, Some(1234));
        assert_eq!(files[0].mtime, Some(1_699_000_000));
        assert_eq!(files[0].mime.as_deref(), Some("application/pdf"));
        assert_eq!(
            files[0].download_url,
            "https://moodle.test/webservice/pluginfile.php/1/hw.pdf"
        );
    }

    fn calendar_event(id: u64, courseid: u64, timeduration: u64, eventtype: &str) -> CalendarEvent {
        CalendarEvent {
            id,
            name: format!("Event {id}"),
            description: String::new(),
            courseid,
            modulename: None,
            instance: None,
            eventtype: eventtype.to_string(),
            timestart: 1_700_000_000,
            timeduration,
            visible: 1,
        }
    }

    #[test]
    fn maps_calendar_event() {
        let event = map_calendar_event("https://moodle.test", &calendar_event(9, 7, 3600, "close"));

        assert_eq!(event.id, "moodle:cal:9");
        assert_eq!(event.source, SourceKind::Moodle);
        assert_eq!(event.kind, EventKind::Calendar);
        assert_eq!(event.title, "Event 9");
        assert_eq!(event.start, Some(1_700_000_000));
        assert_eq!(event.due, Some(1_700_003_600));
        assert_eq!(event.course_id.as_deref(), Some("moodle:7"));
        assert_eq!(event.cmid, None);
        assert_eq!(event.status.as_deref(), Some("close"));
        assert_eq!(event.grade, None);
        assert_eq!(
            event.url.as_deref(),
            Some("https://moodle.test/calendar/view.php?view=day&time=1700000000")
        );
    }

    #[test]
    fn maps_calendar_event_without_duration_or_course() {
        let event = map_calendar_event("https://moodle.test", &calendar_event(10, 0, 0, "user"));
        assert_eq!(event.due, None);
        assert_eq!(event.course_id, None);
        assert_eq!(event.status.as_deref(), Some("user"));
    }

    #[test]
    fn current_assignments() {
        let now = 1_700_000_000;
        assert!(is_current(None, now));
        assert!(is_current(Some(now + 1), now));
        assert!(is_current(Some(now - TODO_PAST_SECS), now));
        assert!(!is_current(Some(now - TODO_PAST_SECS - 1), now));
    }

    #[test]
    fn attachments_by_mimetype() {
        assert!(is_attachment(Some("application/pdf")));
        assert!(is_attachment(Some("text/plain")));
        assert!(is_attachment(None));
        assert!(!is_attachment(Some("text/html; charset=utf-8")));
        assert!(!is_attachment(Some("text/css")));
        assert!(!is_attachment(Some("image/png")));
        assert!(!is_attachment(Some("application/x-javascript")));
    }
}
