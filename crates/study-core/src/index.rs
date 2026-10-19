//! The local metadata snapshot (SPEC §6): a JSON cache so `find` is instant
//! and offline. Deleting it is always safe; `sync` rebuilds it.

use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Result;
use crate::config::Config;
use crate::paths;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    Moodle,
    Teams,
}

impl SourceKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SourceKind::Moodle => "moodle",
            SourceKind::Teams => "teams",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum NodeKind {
    Course,
    Section,
    Module,
    Folder,
    Team,
    Channel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Source {
    pub kind: SourceKind,
    pub name: String,
    pub remote_id: String,
    /// Unix seconds.
    pub last_sync: Option<u64>,
    pub delta_token: Option<String>,
}

/// One entry of the container tree. `id` is `"<source>:<remote_id>"`
/// (e.g. `"moodle:4821"`, `"moodle:mod:9912"`), built by the caller.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Node {
    pub id: String,
    pub source: SourceKind,
    pub parent: Option<String>,
    pub kind: NodeKind,
    pub modname: Option<String>,
    pub name: String,
    pub remote_id: String,
    pub web_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct File {
    pub id: String,
    /// Id of the node this file belongs to.
    pub node: String,
    pub name: String,
    pub size: Option<u64>,
    pub mtime: Option<u64>,
    pub mime: Option<String>,
    pub remote_id: String,
    pub download_url: String,
    pub web_url: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EventKind {
    Calendar,
    Coursework,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Event {
    pub id: String,
    pub source: SourceKind,
    pub kind: EventKind,
    pub title: String,
    pub start: Option<u64>,
    pub due: Option<u64>,
    pub course_id: Option<String>,
    pub cmid: Option<String>,
    pub status: Option<String>,
    pub grade: Option<String>,
    pub url: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Index {
    pub sources: Vec<Source>,
    pub nodes: Vec<Node>,
    pub files: Vec<File>,
    pub events: Vec<Event>,
}

impl Index {
    /// True when any source was never synced or the oldest sync is older
    /// than `max_age_secs`. Decides whether a command auto-syncs first.
    pub fn is_stale(&self, max_age_secs: u64, now: u64) -> bool {
        if self.sources.is_empty() {
            return true;
        }
        let mut oldest = u64::MAX;
        for source in &self.sources {
            match source.last_sync {
                Some(last_sync) => oldest = oldest.min(last_sync),
                None => return true,
            }
        }
        now.saturating_sub(oldest) > max_age_secs
    }

    pub fn courses(&self) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|node| matches!(node.kind, NodeKind::Course | NodeKind::Team))
            .collect()
    }

    pub fn children(&self, node_id: &str) -> Vec<&Node> {
        self.nodes
            .iter()
            .filter(|node| node.parent.as_deref() == Some(node_id))
            .collect()
    }

    pub fn files_in(&self, node_id: &str) -> Vec<&File> {
        self.files
            .iter()
            .filter(|file| file.node == node_id)
            .collect()
    }

    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|node| node.id == id)
    }

    /// Swap in a fresh sync result for one source, leaving the other
    /// source untouched.
    pub fn replace_source(
        &mut self,
        source: Source,
        nodes: Vec<Node>,
        files: Vec<File>,
        events: Vec<Event>,
    ) {
        let kind = source.kind;
        // Files carry no source of their own; they go with their node.
        let removed_nodes: HashSet<&str> = self
            .nodes
            .iter()
            .filter(|node| node.source == kind)
            .map(|node| node.id.as_str())
            .collect();
        self.files
            .retain(|file| !removed_nodes.contains(file.node.as_str()));

        self.sources.retain(|s| s.kind != kind);
        self.nodes.retain(|node| node.source != kind);
        self.events.retain(|event| event.source != kind);

        self.sources.push(source);
        self.nodes.extend(nodes);
        self.files.extend(files);
        self.events.extend(events);
    }
}

pub fn default_path() -> PathBuf {
    paths::data_dir().join("index.json")
}

/// A missing index just means nothing was synced yet.
pub fn load(path: &Path) -> Result<Index> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(Index::default()),
        Err(err) => return Err(err.into()),
    };
    Ok(serde_json::from_slice(&bytes)?)
}

/// Writes a sibling `.tmp` file and renames it over the index, so a crash
/// mid-write never leaves a half-written `index.json`.
pub fn save(index: &Index, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, serde_json::to_vec_pretty(index)?)?;
    fs::rename(&tmp, path)?;
    Ok(())
}

/// Candidate courses for a CLI `<course>` reference: an exact alias wins,
/// then an exact remote id, then a case-insensitive name prefix. More than
/// one candidate means the query is ambiguous; none means not found.
pub fn resolve_course<'a>(index: &'a Index, config: &Config, query: &str) -> Vec<&'a Node> {
    let courses = index.courses();

    let by_alias: Vec<&Node> = config
        .aliases
        .iter()
        .filter(|(_, alias)| alias.as_str() == query)
        .filter_map(|(id, _)| courses.iter().find(|course| course.id == *id).copied())
        .collect();
    if !by_alias.is_empty() {
        return by_alias;
    }

    let by_id: Vec<&Node> = courses
        .iter()
        .filter(|course| course.remote_id == query)
        .copied()
        .collect();
    if !by_id.is_empty() {
        return by_id;
    }

    let query = query.to_lowercase();
    courses
        .into_iter()
        .filter(|course| course.name.to_lowercase().starts_with(&query))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(kind: SourceKind, last_sync: Option<u64>) -> Source {
        Source {
            kind,
            name: "site".to_string(),
            remote_id: "site".to_string(),
            last_sync,
            delta_token: None,
        }
    }

    fn node(
        id: &str,
        source: SourceKind,
        kind: NodeKind,
        parent: Option<&str>,
        name: &str,
    ) -> Node {
        Node {
            id: id.to_string(),
            source,
            parent: parent.map(str::to_string),
            kind,
            modname: None,
            name: name.to_string(),
            remote_id: id.split(':').next_back().unwrap().to_string(),
            web_url: None,
        }
    }

    fn file(id: &str, node: &str) -> File {
        File {
            id: id.to_string(),
            node: node.to_string(),
            name: "f.pdf".to_string(),
            size: Some(1),
            mtime: Some(2),
            mime: None,
            remote_id: id.to_string(),
            download_url: "https://example.edu/f.pdf".to_string(),
            web_url: None,
        }
    }

    fn event(id: &str, source: SourceKind) -> Event {
        Event {
            id: id.to_string(),
            source,
            kind: EventKind::Calendar,
            title: "e".to_string(),
            start: None,
            due: None,
            course_id: None,
            cmid: None,
            status: None,
            grade: None,
            url: None,
        }
    }

    fn sample() -> Index {
        let mut index = Index::default();
        index.replace_source(
            source(SourceKind::Moodle, Some(100)),
            vec![
                node(
                    "moodle:1",
                    SourceKind::Moodle,
                    NodeKind::Course,
                    None,
                    "Algorithms",
                ),
                node(
                    "moodle:2",
                    SourceKind::Moodle,
                    NodeKind::Course,
                    None,
                    "Algebra",
                ),
                node(
                    "moodle:sec:3",
                    SourceKind::Moodle,
                    NodeKind::Section,
                    Some("moodle:1"),
                    "Week 1",
                ),
            ],
            vec![file("moodle:f1", "moodle:sec:3")],
            vec![event("moodle:e1", SourceKind::Moodle)],
        );
        index.replace_source(
            source(SourceKind::Teams, Some(200)),
            vec![node(
                "teams:t1",
                SourceKind::Teams,
                NodeKind::Team,
                None,
                "Machine Learning",
            )],
            vec![file("teams:f1", "teams:t1")],
            vec![event("teams:e1", SourceKind::Teams)],
        );
        index
    }

    #[test]
    fn save_load_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("index.json");
        let index = sample();

        save(&index, &path).unwrap();
        let loaded = load(&path).unwrap();

        assert_eq!(loaded.sources, index.sources);
        assert_eq!(loaded.nodes, index.nodes);
        assert_eq!(loaded.files, index.files);
        assert_eq!(loaded.events, index.events);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn load_missing_gives_default() {
        let dir = tempfile::tempdir().unwrap();
        let index = load(&dir.path().join("index.json")).unwrap();
        assert!(index.sources.is_empty());
        assert!(index.nodes.is_empty());
    }

    #[test]
    fn staleness() {
        assert!(Index::default().is_stale(900, 1000));

        let mut index = sample();
        assert!(!index.is_stale(900, 1000)); // oldest sync is 100, 900 s ago
        assert!(index.is_stale(899, 1000));

        index.sources[0].last_sync = None;
        assert!(index.is_stale(u64::MAX, 1000));
    }

    #[test]
    fn tree_queries() {
        let index = sample();
        let courses: Vec<&str> = index.courses().iter().map(|n| n.id.as_str()).collect();
        assert_eq!(courses, ["moodle:1", "moodle:2", "teams:t1"]);
        assert_eq!(index.children("moodle:1")[0].id, "moodle:sec:3");
        assert!(index.children("moodle:2").is_empty());
        assert_eq!(index.files_in("moodle:sec:3")[0].id, "moodle:f1");
        assert_eq!(index.node("moodle:2").unwrap().name, "Algebra");
        assert!(index.node("nope").is_none());
    }

    #[test]
    fn replace_source_keeps_other_source() {
        let mut index = sample();
        index.replace_source(
            source(SourceKind::Moodle, Some(300)),
            vec![node(
                "moodle:9",
                SourceKind::Moodle,
                NodeKind::Course,
                None,
                "New",
            )],
            vec![],
            vec![],
        );

        let node_ids: Vec<&str> = index.nodes.iter().map(|n| n.id.as_str()).collect();
        assert_eq!(node_ids, ["teams:t1", "moodle:9"]);
        let file_ids: Vec<&str> = index.files.iter().map(|f| f.id.as_str()).collect();
        assert_eq!(file_ids, ["teams:f1"]);
        let event_ids: Vec<&str> = index.events.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(event_ids, ["teams:e1"]);
        assert_eq!(index.sources.len(), 2);
        assert_eq!(index.sources[1].last_sync, Some(300));
    }

    #[test]
    fn resolve_alias_then_id_then_prefix() {
        let index = sample();
        let mut config = Config::default();
        config.set_alias("teams:t1", "algo");

        // The alias "algo" wins over the name prefix "Algo…".
        let found = resolve_course(&index, &config, "algo");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "teams:t1");

        let found = resolve_course(&index, &config, "2");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "moodle:2");

        let found = resolve_course(&index, &config, "machine");
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].id, "teams:t1");

        assert!(resolve_course(&index, &config, "nothing").is_empty());
    }

    #[test]
    fn resolve_reports_ambiguity() {
        let index = sample();
        let found = resolve_course(&index, &Config::default(), "alg");
        assert_eq!(found.len(), 2);
    }
}
