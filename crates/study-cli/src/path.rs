//! `<course>/<segment>/...` references → index nodes and files (SPEC §3).

use anyhow::{Result, bail};
use study_core::config::Config;
use study_core::index::{File, Index, Node, NodeKind, resolve_course};

#[derive(Debug, Clone, Copy)]
pub enum Target<'a> {
    Node(&'a Node),
    File(&'a File),
}

impl Target<'_> {
    pub fn name(&self) -> &str {
        match self {
            Target::Node(node) => &node.name,
            Target::File(file) => &file.name,
        }
    }
}

/// Resolves a reference to exactly one target. Ambiguity lists the
/// candidates on stderr and exits with code 2, as every command wants.
pub fn resolve<'a>(index: &'a Index, config: &Config, reference: &str) -> Result<Target<'a>> {
    let found = lookup(index, config, reference)?;
    if let [target] = found[..] {
        return Ok(target);
    }
    eprintln!("'{reference}' is ambiguous — did you mean:");
    for target in &found {
        eprintln!("  {}", label(target, config));
    }
    std::process::exit(2)
}

/// Every node from the course down to (and including) `node_id`.
pub fn ancestors<'a>(index: &'a Index, node_id: &str) -> Vec<&'a Node> {
    let mut chain = Vec::new();
    let mut current = index.node(node_id);
    while let Some(node) = current {
        chain.push(node);
        current = node.parent.as_deref().and_then(|id| index.node(id));
    }
    chain.reverse();
    chain
}

/// Walks the segments; returns one target when resolved, several when a
/// segment is ambiguous (the walk stops there), an error when nothing matches.
fn lookup<'a>(index: &'a Index, config: &Config, reference: &str) -> Result<Vec<Target<'a>>> {
    let segments: Vec<&str> = reference.split('/').filter(|s| !s.is_empty()).collect();
    let Some((course, rest)) = segments.split_first() else {
        bail!("empty reference");
    };
    let courses = resolve_course(index, config, course);
    if courses.is_empty() {
        bail!("no course matches '{course}'");
    }
    let mut found: Vec<Target> = courses.into_iter().map(Target::Node).collect();

    for (i, segment) in rest.iter().enumerate() {
        let node = match found[..] {
            [Target::Node(node)] => node,
            [Target::File(file)] => bail!("'{}' is a file, not a folder", file.name),
            _ => return Ok(found),
        };
        let last = i + 1 == rest.len();
        found = step(index, node, segment, last);
        if found.is_empty() {
            bail!("nothing matches '{segment}' in '{}'", node.name);
        }
    }
    Ok(found)
}

/// Candidates for one segment: child nodes, plus files on the last segment.
fn step<'a>(index: &'a Index, node: &Node, segment: &str, last: bool) -> Vec<Target<'a>> {
    let mut candidates: Vec<Target> = index
        .children(&node.id)
        .into_iter()
        .map(Target::Node)
        .collect();
    if last {
        candidates.extend(index.files_in(&node.id).into_iter().map(Target::File));
    }
    let names: Vec<&str> = candidates.iter().map(Target::name).collect();
    matching(&names, segment)
        .into_iter()
        .map(|i| candidates[i])
        .collect()
}

/// Indices of the names matching `query` case-insensitively: exact matches
/// if there are any, otherwise prefix matches.
fn matching(names: &[&str], query: &str) -> Vec<usize> {
    let query = query.to_lowercase();
    let lower: Vec<String> = names.iter().map(|name| name.to_lowercase()).collect();
    let exact: Vec<usize> = (0..names.len()).filter(|&i| lower[i] == query).collect();
    if !exact.is_empty() {
        return exact;
    }
    (0..names.len())
        .filter(|&i| lower[i].starts_with(&query))
        .collect()
}

fn label(target: &Target, config: &Config) -> String {
    match target {
        Target::File(file) => file.name.clone(),
        Target::Node(node) if node.kind == NodeKind::Course => match config.alias_for(&node.id) {
            Some(alias) => format!("{alias}  ({})", node.name),
            None => node.name.clone(),
        },
        Target::Node(node) => format!("{}/", node.name),
    }
}

#[cfg(test)]
mod tests {
    use study_core::index::{Source, SourceKind};

    use super::*;

    fn node(id: &str, kind: NodeKind, parent: Option<&str>, name: &str) -> Node {
        Node {
            id: id.to_string(),
            source: SourceKind::Moodle,
            parent: parent.map(str::to_string),
            kind,
            modname: None,
            name: name.to_string(),
            remote_id: id.rsplit(':').next().unwrap().to_string(),
            web_url: None,
        }
    }

    fn file(id: &str, node: &str, name: &str) -> File {
        File {
            id: id.to_string(),
            node: node.to_string(),
            name: name.to_string(),
            size: None,
            mtime: None,
            mime: None,
            remote_id: id.to_string(),
            download_url: format!("https://moodle.test/{name}"),
            web_url: None,
        }
    }

    fn sample() -> (Index, Config) {
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
                    "moodle:sec:4",
                    NodeKind::Section,
                    Some("moodle:1"),
                    "Week 10",
                ),
                node(
                    "moodle:mod:5",
                    NodeKind::Folder,
                    Some("moodle:sec:3"),
                    "Exercises",
                ),
            ],
            vec![
                file("f1", "moodle:sec:3", "Lecture1.pdf"),
                file("f2", "moodle:sec:3", "Lecture1-notes.pdf"),
                file("f3", "moodle:mod:5", "ex1.pdf"),
            ],
            vec![],
        );
        let mut config = Config::default();
        config.set_alias("moodle:2", "lin");
        (index, config)
    }

    #[test]
    fn course_by_alias_and_prefix() {
        let (index, config) = sample();
        assert_eq!(resolve(&index, &config, "lin").unwrap().name(), "Algebra");
        assert_eq!(
            resolve(&index, &config, "algor").unwrap().name(),
            "Algorithms"
        );
        assert!(resolve(&index, &config, "nothing").is_err());
    }

    #[test]
    fn exact_wins_over_prefix() {
        let (index, config) = sample();
        // "week 1" is a prefix of "Week 10" too, but matches "Week 1" exactly.
        let found = lookup(&index, &config, "algorithms/week 1").unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name(), "Week 1");
        assert_eq!(
            matching(&["Lecture1.pdf", "Lecture1-notes.pdf"], "lecture1"),
            [0, 1]
        );
        assert_eq!(
            matching(&["Lecture1.pdf", "Lecture1-notes.pdf"], "lecture1.pdf"),
            [0]
        );
    }

    #[test]
    fn ambiguity_is_detected() {
        let (index, config) = sample();
        let found = lookup(&index, &config, "alg").unwrap();
        assert_eq!(found.len(), 2);
        let found = lookup(&index, &config, "algorithms/week").unwrap();
        assert_eq!(found.len(), 2);
        let found = lookup(&index, &config, "algorithms/week 1/lecture1").unwrap();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn walks_down_to_a_file() {
        let (index, config) = sample();
        let target = resolve(&index, &config, "algorithms/week 1/exercises/ex1").unwrap();
        let Target::File(file) = target else {
            panic!("expected a file")
        };
        assert_eq!(file.id, "f3");

        let target = resolve(&index, &config, "algorithms/week 1/exer").unwrap();
        let Target::Node(folder) = target else {
            panic!("expected a node")
        };
        assert_eq!(folder.id, "moodle:mod:5");
    }

    #[test]
    fn files_only_match_on_the_last_segment() {
        let (index, config) = sample();
        assert!(lookup(&index, &config, "algorithms/week 1/ex1.pdf/x").is_err());
        assert!(lookup(&index, &config, "algorithms/week 1/lecture1-notes.pdf").is_ok());
        assert!(lookup(&index, &config, "algorithms/week 1/missing").is_err());
    }

    #[test]
    fn ancestors_run_root_first() {
        let (index, _) = sample();
        let chain: Vec<&str> = ancestors(&index, "moodle:mod:5")
            .iter()
            .map(|n| n.name.as_str())
            .collect();
        assert_eq!(chain, ["Algorithms", "Week 1", "Exercises"]);
    }
}
