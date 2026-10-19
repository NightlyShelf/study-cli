//! `study sync` — crawl Moodle into the index (SPEC §6).

use std::collections::HashSet;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use indicatif::ProgressBar;
use study_core::config::Config;
use study_core::index::{File, Index, Node, NodeKind, Source, SourceKind};
use study_moodle::api::{Content, Course, Module, Section};

use crate::app;

pub async fn run() -> Result<()> {
    let config = app::load_config()?;
    sync(&config).await?;
    Ok(())
}

/// Full crawl: site info → courses → contents of each course. ~10 round-trips,
/// so a sequential loop is fine.
pub async fn sync(config: &Config) -> Result<Index> {
    let client = app::moodle_client(config)?;
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

    let spinner = ProgressBar::new_spinner();
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
    spinner.finish_and_clear();
    eprintln!("synced {} courses, {} files", courses.len(), files.len());

    let mut index = app::load_index()?;
    let source = Source {
        kind: SourceKind::Moodle,
        name: site.sitename,
        remote_id: client.base_url.clone(),
        last_sync: Some(app::now()),
        delta_token: None,
    };
    index.replace_source(source, nodes, files, Vec::new());
    app::save_index(&index)?;
    Ok(index)
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
