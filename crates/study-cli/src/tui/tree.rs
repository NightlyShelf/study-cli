//! The flat list of lines the Contents pane shows, built from the index.

use std::collections::HashSet;
use std::path::PathBuf;

use study_core::index::{File, Index, Node, NodeKind};

/// One visible line of the content tree.
#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: String,
    pub depth: u8,
    pub kind: RowKind,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RowKind {
    /// A section, folder or module, with the module's `modname`.
    Node(NodeKind, Option<String>),
    /// A file with its size.
    File(Option<u64>),
}

impl Row {
    pub fn is_node(&self) -> bool {
        matches!(self.kind, RowKind::Node(..))
    }

    pub fn modname(&self) -> Option<&str> {
        match &self.kind {
            RowKind::Node(_, modname) => modname.as_deref(),
            RowKind::File(_) => None,
        }
    }
}

/// The rows of one course, depth-first: a node, then (when open) its files,
/// then its child nodes. A node is open iff `expanded` has its id. With a
/// filter every node is walked regardless of `expanded` and only the rows
/// whose name contains the filter survive, together with their ancestors.
/// Files on the course itself (a single-channel team, SPEC §4.2) come first.
pub fn build_rows(
    index: &Index,
    course_id: &str,
    expanded: &HashSet<String>,
    filter: &str,
) -> Vec<Row> {
    let filter = filter.to_lowercase();
    let mut rows = Vec::new();
    push_files(index, course_id, 0, &filter, &mut rows);
    for child in index.children(course_id) {
        push_node(index, child, 0, expanded, &filter, &mut rows);
    }
    rows
}

fn push_files(index: &Index, node_id: &str, depth: u8, filter: &str, rows: &mut Vec<Row>) {
    for file in index.files_in(node_id) {
        if !matches(&file.name, filter) {
            continue;
        }
        rows.push(Row {
            id: file.id.clone(),
            depth,
            kind: RowKind::File(file.size),
            name: file.name.clone(),
        });
    }
}

fn push_node(
    index: &Index,
    node: &Node,
    depth: u8,
    expanded: &HashSet<String>,
    filter: &str,
    rows: &mut Vec<Row>,
) {
    let start = rows.len();
    rows.push(Row {
        id: node.id.clone(),
        depth,
        kind: RowKind::Node(node.kind, node.modname.clone()),
        name: node.name.clone(),
    });
    let open = !filter.is_empty() || expanded.contains(&node.id);
    if !open {
        return;
    }
    push_files(index, &node.id, depth + 1, filter, rows);
    for child in index.children(&node.id) {
        push_node(index, child, depth + 1, expanded, filter, rows);
    }
    // A node that neither matches nor has a matching descendant is noise.
    if !filter.is_empty() && rows.len() == start + 1 && !matches(&node.name, filter) {
        rows.pop();
    }
}

fn matches(name: &str, filter: &str) -> bool {
    filter.is_empty() || name.to_lowercase().contains(filter)
}

/// True when opening the node would show anything.
pub fn has_children(index: &Index, node_id: &str) -> bool {
    !index.children(node_id).is_empty() || !index.files_in(node_id).is_empty()
}

/// Every file below `node` with the directory it belongs in, mirroring node
/// names (`<node>/<child>/…`) exactly like `study get`. Owned copies, so a
/// background task can take them.
pub fn files_under(index: &Index, node: &Node) -> Vec<(File, PathBuf)> {
    let mut found = Vec::new();
    let mut pending = vec![(node, PathBuf::from(&node.name))];
    while let Some((node, dir)) = pending.pop() {
        for file in index.files_in(&node.id) {
            found.push((file.clone(), dir.clone()));
        }
        for child in index.children(&node.id).into_iter().rev() {
            pending.push((child, dir.join(&child.name)));
        }
    }
    found
}

#[cfg(test)]
pub mod tests {
    use study_core::index::{Source, SourceKind};

    use super::*;

    pub fn node(id: &str, kind: NodeKind, parent: Option<&str>, name: &str) -> Node {
        Node {
            id: id.to_string(),
            source: SourceKind::Moodle,
            parent: parent.map(str::to_string),
            kind,
            modname: match kind {
                NodeKind::Folder => Some("folder".to_string()),
                NodeKind::Module => Some("page".to_string()),
                _ => None,
            },
            name: name.to_string(),
            remote_id: id.rsplit(':').next().unwrap().to_string(),
            web_url: Some(format!("https://moodle.test/{id}")),
        }
    }

    pub fn file(id: &str, node: &str, name: &str) -> File {
        File {
            id: id.to_string(),
            node: node.to_string(),
            name: name.to_string(),
            size: Some(2048),
            mtime: Some(1_700_000_000),
            mime: Some("application/pdf".to_string()),
            remote_id: id.to_string(),
            download_url: format!("https://moodle.test/pluginfile.php/{name}"),
            web_url: None,
        }
    }

    /// Algorithms: Week 1 (Lecture1.pdf, Exercises/ (ex1.pdf), Syllabus page),
    /// Week 2 (Lecture2.pdf). Algebra: nothing.
    pub fn sample() -> Index {
        let mut index = Index::default();
        index.replace_source(
            Source {
                kind: SourceKind::Moodle,
                name: "site".to_string(),
                remote_id: "site".to_string(),
                last_sync: Some(1),
                delta_token: None,
            },
            vec![
                node("moodle:1", NodeKind::Course, None, "Algorithms"),
                node("moodle:2", NodeKind::Course, None, "Algebra"),
                node(
                    "moodle:sec:3",
                    NodeKind::Section,
                    Some("moodle:1"),
                    "Week 1",
                ),
                node(
                    "moodle:mod:5",
                    NodeKind::Folder,
                    Some("moodle:sec:3"),
                    "Exercises",
                ),
                node(
                    "moodle:mod:6",
                    NodeKind::Module,
                    Some("moodle:sec:3"),
                    "Syllabus",
                ),
                node(
                    "moodle:sec:4",
                    NodeKind::Section,
                    Some("moodle:1"),
                    "Week 2",
                ),
            ],
            vec![
                file("f1", "moodle:sec:3", "Lecture1.pdf"),
                file("f2", "moodle:mod:5", "ex1.pdf"),
                file("f3", "moodle:sec:4", "Lecture2.pdf"),
            ],
            vec![],
        );
        index
    }

    /// A Teams node: ids follow the sync layout (`teams:<team>`,
    /// `teams:chan:<channel>`, `teams:msg:<channel>`).
    fn teams_node(
        id: &str,
        kind: NodeKind,
        parent: Option<&str>,
        name: &str,
        remote_id: &str,
    ) -> Node {
        Node {
            id: id.to_string(),
            source: SourceKind::Teams,
            parent: parent.map(str::to_string),
            kind,
            modname: (kind == NodeKind::Module).then(|| "messages".to_string()),
            name: name.to_string(),
            remote_id: remote_id.to_string(),
            web_url: Some(format!("https://teams.microsoft.com/l/{id}")),
        }
    }

    /// Two teams: Machine Learning has only General (slides.pdf + its
    /// messages, straight under the team); Databases has Labs and Exams
    /// channels, each with a file and its own messages entry.
    pub fn teams() -> Index {
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
                teams_node("teams:T1", NodeKind::Team, None, "Machine Learning", "T1"),
                teams_node(
                    "teams:msg:C1",
                    NodeKind::Module,
                    Some("teams:T1"),
                    "Messages (General)",
                    "T1/C1",
                ),
                teams_node("teams:T2", NodeKind::Team, None, "Databases", "T2"),
                teams_node(
                    "teams:chan:C2",
                    NodeKind::Channel,
                    Some("teams:T2"),
                    "Labs",
                    "C2",
                ),
                teams_node(
                    "teams:msg:C2",
                    NodeKind::Module,
                    Some("teams:chan:C2"),
                    "Messages (Labs)",
                    "T2/C2",
                ),
                teams_node(
                    "teams:chan:C3",
                    NodeKind::Channel,
                    Some("teams:T2"),
                    "Exams",
                    "C3",
                ),
                teams_node(
                    "teams:msg:C3",
                    NodeKind::Module,
                    Some("teams:chan:C3"),
                    "Messages (Exams)",
                    "T2/C3",
                ),
            ],
            vec![
                file("teams:file:I1", "teams:T1", "slides.pdf"),
                file("teams:file:I2", "teams:chan:C2", "lab1.pdf"),
                file("teams:file:I3", "teams:chan:C3", "mock.pdf"),
            ],
            vec![],
        );
        index
    }

    fn names(rows: &[Row]) -> Vec<String> {
        rows.iter()
            .map(|row| format!("{}{}", " ".repeat(row.depth as usize), row.name))
            .collect()
    }

    fn sections() -> HashSet<String> {
        ["moodle:sec:3", "moodle:sec:4"]
            .iter()
            .map(|id| id.to_string())
            .collect()
    }

    #[test]
    fn open_sections_show_files_then_child_nodes() {
        let rows = build_rows(&sample(), "moodle:1", &sections(), "");
        assert_eq!(
            names(&rows),
            [
                "Week 1",
                " Lecture1.pdf",
                " Exercises",
                " Syllabus",
                "Week 2",
                " Lecture2.pdf"
            ]
        );
        assert_eq!(rows[1].kind, RowKind::File(Some(2048)));
        assert_eq!(rows[2].modname(), Some("folder"));
    }

    #[test]
    fn closed_nodes_hide_their_contents() {
        let rows = build_rows(&sample(), "moodle:1", &HashSet::new(), "");
        assert_eq!(names(&rows), ["Week 1", "Week 2"]);

        let mut expanded = sections();
        expanded.insert("moodle:mod:5".to_string());
        let rows = build_rows(&sample(), "moodle:1", &expanded, "");
        assert_eq!(names(&rows)[2..4], [" Exercises", "  ex1.pdf"]);
    }

    #[test]
    fn filter_keeps_matches_and_ancestors_and_ignores_expansion() {
        let rows = build_rows(&sample(), "moodle:1", &HashSet::new(), "EX1");
        assert_eq!(names(&rows), ["Week 1", " Exercises", "  ex1.pdf"]);

        let rows = build_rows(&sample(), "moodle:1", &HashSet::new(), "lecture");
        assert_eq!(
            names(&rows),
            ["Week 1", " Lecture1.pdf", "Week 2", " Lecture2.pdf"]
        );

        // A matching node is kept even when nothing below it matches.
        let rows = build_rows(&sample(), "moodle:1", &HashSet::new(), "week 2");
        assert_eq!(names(&rows), ["Week 2"]);

        assert!(build_rows(&sample(), "moodle:1", &HashSet::new(), "zzz").is_empty());
    }

    #[test]
    fn single_channel_team_shows_files_and_messages_at_the_top() {
        let index = teams();
        let rows = build_rows(&index, "teams:T1", &HashSet::new(), "");
        assert_eq!(names(&rows), ["slides.pdf", "Messages (General)"]);
        assert_eq!(rows[0].kind, RowKind::File(Some(2048)));
        assert_eq!(rows[1].modname(), Some("messages"));
        assert!(!has_children(&index, "teams:msg:C1"));
        // The filter applies to the team's own files too.
        let rows = build_rows(&index, "teams:T1", &HashSet::new(), "slides");
        assert_eq!(names(&rows), ["slides.pdf"]);
    }

    #[test]
    fn multi_channel_team_shows_channel_folders_with_messages() {
        let index = teams();
        let closed = build_rows(&index, "teams:T2", &HashSet::new(), "");
        assert_eq!(names(&closed), ["Labs", "Exams"]);
        assert_eq!(closed[0].kind, RowKind::Node(NodeKind::Channel, None));
        assert!(has_children(&index, "teams:chan:C2"));

        let expanded: HashSet<String> = ["teams:chan:C2", "teams:chan:C3"]
            .iter()
            .map(|id| id.to_string())
            .collect();
        let rows = build_rows(&index, "teams:T2", &expanded, "");
        assert_eq!(
            names(&rows),
            [
                "Labs",
                " lab1.pdf",
                " Messages (Labs)",
                "Exams",
                " mock.pdf",
                " Messages (Exams)"
            ]
        );
        assert_eq!(rows[2].modname(), Some("messages"));
    }

    #[test]
    fn empty_course_has_no_rows() {
        assert!(build_rows(&sample(), "moodle:2", &sections(), "").is_empty());
        assert!(build_rows(&sample(), "missing", &sections(), "").is_empty());
    }

    #[test]
    fn files_under_mirrors_node_names() {
        let index = sample();
        let found: Vec<(String, PathBuf)> =
            files_under(&index, index.node("moodle:sec:3").unwrap())
                .into_iter()
                .map(|(file, dir)| (file.name, dir))
                .collect();
        assert_eq!(
            found,
            [
                ("Lecture1.pdf".to_string(), PathBuf::from("Week 1")),
                ("ex1.pdf".to_string(), PathBuf::from("Week 1/Exercises")),
            ]
        );
        assert!(has_children(&index, "moodle:sec:3"));
        assert!(!has_children(&index, "moodle:mod:6"));
    }

    /// The real index, when this machine has one: every course must build
    /// without panicking and the first one must show something.
    #[test]
    fn real_index_builds() {
        let Ok(index) = study_core::index::load(&study_core::index::default_path()) else {
            return;
        };
        let courses = index.courses();
        let Some(first) = courses.first() else {
            return;
        };
        let expanded: HashSet<String> = index
            .nodes
            .iter()
            .filter(|node| node.kind == NodeKind::Section)
            .map(|node| node.id.clone())
            .collect();
        for course in &courses {
            build_rows(&index, &course.id, &expanded, "");
            build_rows(&index, &course.id, &HashSet::new(), "a");
        }
        assert!(!build_rows(&index, &first.id, &expanded, "").is_empty());
    }
}
