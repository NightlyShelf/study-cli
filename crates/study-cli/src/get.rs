//! `study get <REF>` — download one file or everything under a node.

use std::path::PathBuf;

use anyhow::{Result, bail};
use study_core::index::{File, Index, Node, NodeKind};

use crate::path::{self, Target};
use crate::{app, download};

pub async fn run(
    reference: &str,
    dir: Option<PathBuf>,
    force: bool,
    overwrite: bool,
) -> Result<()> {
    let config = app::load_config()?;
    let index = app::fresh_index(&config).await?;
    let dir = match dir {
        Some(dir) => dir,
        None => std::env::current_dir()?,
    };

    let client = app::moodle_client(&config)?;
    let node = match path::resolve(&index, &config, reference)? {
        Target::File(file) => {
            download::download(&client, file, &dir, force, overwrite, false).await?;
            return Ok(());
        }
        Target::Node(node) => node,
    };
    if node.kind == NodeKind::Course {
        bail!(
            "'{reference}' is a whole course — pick a section or folder (see `study ls {reference}`)"
        );
    }
    for (file, subdir) in files_under(&index, node) {
        download::download(&client, file, &dir.join(subdir), force, overwrite, false).await?;
    }
    Ok(())
}

/// Every file below `node` with the directory it belongs in, mirroring node
/// names (`<node>/<child>/…`). Iterative so the async download loop needs
/// no recursion.
fn files_under<'a>(index: &'a Index, node: &'a Node) -> Vec<(&'a File, PathBuf)> {
    let mut found = Vec::new();
    let mut pending = vec![(node, PathBuf::from(&node.name))];
    while let Some((node, dir)) = pending.pop() {
        for file in index.files_in(&node.id) {
            found.push((file, dir.clone()));
        }
        for child in index.children(&node.id).into_iter().rev() {
            pending.push((child, dir.join(&child.name)));
        }
    }
    found
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
            remote_id: id.to_string(),
            web_url: None,
        }
    }

    fn file(node: &str, name: &str) -> File {
        File {
            id: format!("{node}:{name}"),
            node: node.to_string(),
            name: name.to_string(),
            size: None,
            mtime: None,
            mime: None,
            remote_id: name.to_string(),
            download_url: name.to_string(),
            web_url: None,
        }
    }

    #[test]
    fn walks_own_files_then_children_in_order() {
        let mut index = Index::default();
        index.replace_source(
            Source {
                kind: SourceKind::Moodle,
                name: "s".to_string(),
                remote_id: "s".to_string(),
                last_sync: None,
                delta_token: None,
            },
            vec![
                node("c", NodeKind::Course, None, "Course"),
                node("s1", NodeKind::Section, Some("c"), "Week 1"),
                node("f1", NodeKind::Folder, Some("s1"), "Exercises"),
                node("f2", NodeKind::Folder, Some("s1"), "Solutions"),
            ],
            vec![
                file("s1", "slides.pdf"),
                file("f1", "ex1.pdf"),
                file("f2", "sub/sol1.pdf"),
            ],
            vec![],
        );
        let section = index.node("s1").unwrap();
        let found: Vec<(String, PathBuf)> = files_under(&index, section)
            .into_iter()
            .map(|(file, dir)| (file.name.clone(), dir))
            .collect();
        assert_eq!(
            found,
            [
                ("slides.pdf".to_string(), PathBuf::from("Week 1")),
                ("ex1.pdf".to_string(), PathBuf::from("Week 1/Exercises")),
                (
                    "sub/sol1.pdf".to_string(),
                    PathBuf::from("Week 1/Solutions")
                ),
            ]
        );
    }
}
