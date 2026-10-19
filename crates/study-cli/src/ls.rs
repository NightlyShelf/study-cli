//! `study ls [REF]` — courses, or the contents of a node, from the index.

use anyhow::Result;
use study_core::config::Config;
use study_core::index::{File, Index, Node};

use crate::app;
use crate::fmt::{human_age, human_size, print_rows};
use crate::path::{self, Target};

pub async fn run(reference: Option<String>) -> Result<()> {
    let config = app::load_config()?;
    let index = app::fresh_index(&config).await?;
    let Some(reference) = reference else {
        list_courses(&index, &config);
        return Ok(());
    };
    match path::resolve(&index, &config, &reference)? {
        Target::Node(node) => list_node(&index, node),
        Target::File(file) => show_file(&index, file),
    }
    Ok(())
}

fn list_courses(index: &Index, config: &Config) {
    let rows: Vec<Vec<String>> = index
        .courses()
        .iter()
        .map(|course| {
            vec![
                config.alias_for(&course.id).unwrap_or("-").to_string(),
                course.name.clone(),
                course.source.as_str().to_string(),
            ]
        })
        .collect();
    print_rows(&rows);
}

/// Child nodes first (marked with a trailing `/`), then files.
fn list_node(index: &Index, node: &Node) {
    let now = app::now();
    let mut rows: Vec<Vec<String>> = index
        .children(&node.id)
        .iter()
        .map(|child| vec![format!("{}/", child.name), String::new(), String::new()])
        .collect();
    for file in index.files_in(&node.id) {
        rows.push(vec![
            file.name.clone(),
            human_size(file.size),
            human_age(file.mtime, now),
        ]);
    }
    if rows.is_empty() {
        eprintln!("(empty)");
    }
    print_rows(&rows);
}

fn show_file(index: &Index, file: &File) {
    let location: Vec<&str> = path::ancestors(index, &file.node)
        .iter()
        .map(|node| node.name.as_str())
        .collect();
    let rows = vec![
        vec!["name".to_string(), file.name.clone()],
        vec!["in".to_string(), location.join("/")],
        vec!["size".to_string(), human_size(file.size)],
        vec!["modified".to_string(), human_age(file.mtime, app::now())],
        vec!["mime".to_string(), file.mime.clone().unwrap_or_default()],
        vec!["url".to_string(), file.download_url.clone()],
    ];
    print_rows(&rows);
}
