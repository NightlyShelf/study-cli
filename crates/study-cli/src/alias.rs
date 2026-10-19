//! `study alias <COURSE> [ALIAS]` — set or show a course's short alias (SPEC §4.1).

use anyhow::{Result, bail};
use study_core::index::NodeKind;

use crate::app;
use crate::path::{self, Target};

pub async fn run(course: &str, alias: Option<&str>) -> Result<()> {
    let mut config = app::load_config()?;
    let index = app::fresh_index(&config).await?;
    let target = path::resolve(&index, &config, course)?;
    let Target::Node(node) = target else {
        bail!("'{course}' is a file, not a course");
    };
    if node.kind != NodeKind::Course {
        bail!("'{}' is not a course", node.name);
    }

    let Some(alias) = alias else {
        println!("{}", config.alias_for(&node.id).unwrap_or("(none)"));
        return Ok(());
    };
    config.set_alias(&node.id, alias);
    app::save_config(&config)?;
    println!("alias: {alias} → {}", node.name);
    Ok(())
}
