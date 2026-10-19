//! `study find <QUERY>...` — fuzzy search over every indexed file.

use std::io::{self, BufRead, IsTerminal, Write};

use anyhow::{Result, bail};
use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use study_core::index::{File, Index};

use crate::fmt::{human_size, print_rows};
use crate::{app, download, path};

const MAX_SHOWN: usize = 20;

struct Hit<'a> {
    file: &'a File,
    /// `course/section[/folder]`, without the file name.
    location: String,
}

pub async fn run(query: &str) -> Result<()> {
    let config = app::load_config()?;
    let index = app::fresh_index(&config).await?;
    let hits = search(&index, query);
    if hits.is_empty() {
        bail!("no files match '{query}'");
    }

    let names: Vec<&str> = hits.iter().map(|hit| hit.file.name.as_str()).collect();
    let chosen = match single_choice(&names, query) {
        Some(i) => i,
        None => match pick(&hits)? {
            Some(i) => i,
            None => return Ok(()),
        },
    };

    let client = app::moodle_client(&config)?;
    let dir = std::env::current_dir()?;
    download::download(&client, hits[chosen].file, &dir, false, false, false).await?;
    Ok(())
}

/// Every file as a hit, ranked by [`tiered`]; best score first.
fn search<'a>(index: &'a Index, query: &str) -> Vec<Hit<'a>> {
    let locations: Vec<String> = index
        .files
        .iter()
        .map(|file| {
            let names: Vec<&str> = path::ancestors(index, &file.node)
                .iter()
                .map(|node| node.name.as_str())
                .collect();
            names.join("/")
        })
        .collect();
    let paths: Vec<String> = index
        .files
        .iter()
        .zip(&locations)
        .map(|(file, location)| format!("{location}/{}", file.name))
        .collect();
    let candidates: Vec<(&str, &str)> = index
        .files
        .iter()
        .zip(&paths)
        .map(|(file, path)| (file.name.as_str(), path.as_str()))
        .collect();

    tiered(&candidates, query)
        .into_iter()
        .map(|(i, _)| Hit {
            file: &index.files[i],
            location: locations[i].clone(),
        })
        .collect()
}

/// `(index, score)` of the matching `(name, full path)` candidates, best
/// first, from the first non-empty tier: the query as a substring of the
/// name, a fuzzy match on the name, a fuzzy match on the whole path. Fuzzy
/// matching the path alone made "lecture" hit a `.css` file in a
/// "SELECT – funkcje…" section, hence the tiers.
fn tiered(candidates: &[(&str, &str)], query: &str) -> Vec<(usize, u32)> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();
    let mut score = |text: &str| pattern.score(Utf32Str::new(text, &mut buf), &mut matcher);
    let needle = query.to_lowercase();

    let mut substring = Vec::new();
    let mut by_name = Vec::new();
    let mut by_path = Vec::new();
    for (i, (name, path)) in candidates.iter().enumerate() {
        if name.to_lowercase().contains(&needle) {
            substring.push((i, score(name).unwrap_or(0)));
        } else if let Some(score) = score(name) {
            by_name.push((i, score));
        } else if let Some(score) = score(path) {
            by_path.push((i, score));
        }
    }

    let mut tier = match (substring.is_empty(), by_name.is_empty()) {
        (false, _) => substring,
        (true, false) => by_name,
        (true, true) => by_path,
    };
    tier.sort_by_key(|&(_, score)| std::cmp::Reverse(score));
    tier
}

/// The one hit to download without asking: the only match, or the only
/// file whose name is exactly the query (SPEC §3).
fn single_choice(names: &[&str], query: &str) -> Option<usize> {
    if names.len() == 1 {
        return Some(0);
    }
    let query = query.to_lowercase();
    let exact: Vec<usize> = (0..names.len())
        .filter(|&i| names[i].to_lowercase() == query)
        .collect();
    match exact[..] {
        [only] => Some(only),
        _ => None,
    }
}

/// Interactive: numbered list and a prompt; `None` when cancelled.
/// Piped: the matches as TSV, exit 2 so scripts can tell "pick one" apart.
fn pick(hits: &[Hit]) -> Result<Option<usize>> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        let rows: Vec<Vec<String>> = hits.iter().map(|hit| row(hit, None)).collect();
        print_rows(&rows);
        std::process::exit(2)
    }

    let shown = hits.len().min(MAX_SHOWN);
    let rows: Vec<Vec<String>> = hits[..shown]
        .iter()
        .enumerate()
        .map(|(i, hit)| row(hit, Some(i + 1)))
        .collect();
    print_rows(&rows);
    if hits.len() > shown {
        println!("… and {} more; refine the query", hits.len() - shown);
    }

    eprint!("Download which? [1-{shown}, or Enter to cancel]: ");
    io::stderr().flush()?;
    let mut line = String::new();
    io::stdin().lock().read_line(&mut line)?;
    let Ok(number) = line.trim().parse::<usize>() else {
        return Ok(None);
    };
    if number == 0 || number > shown {
        return Ok(None);
    }
    Ok(Some(number - 1))
}

fn row(hit: &Hit, number: Option<usize>) -> Vec<String> {
    let mut row = Vec::new();
    if let Some(number) = number {
        row.push(format!("{number}."));
    }
    row.push(hit.location.clone());
    row.push(hit.file.name.clone());
    row.push(human_size(hit.file.size));
    row
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two files in one section; only the section name mentions SELECT.
    fn candidates() -> Vec<(&'static str, &'static str)> {
        vec![
            (
                "Lecture5_Graphs.pdf",
                "Databases/05 – SELECT – funkcje agregujące/Lecture5_Graphs.pdf",
            ),
            (
                "tocbot.css",
                "Databases/05 – SELECT – funkcje agregujące/tocbot.css",
            ),
        ]
    }

    fn found(candidates: &[(&str, &str)], query: &str) -> Vec<usize> {
        let mut hits: Vec<usize> = tiered(candidates, query)
            .into_iter()
            .map(|(i, _)| i)
            .collect();
        hits.sort_unstable();
        hits
    }

    #[test]
    fn name_tiers_beat_the_path() {
        let candidates = candidates();
        assert_eq!(found(&candidates, "lecture"), [0]);
        assert_eq!(found(&candidates, "lec"), [0]);
        assert_eq!(found(&candidates, "toc"), [1]);
        assert!(found(&candidates, "nothing-here").is_empty());
    }

    #[test]
    fn path_is_the_last_resort() {
        let mut candidates = candidates();
        assert_eq!(found(&candidates, "select"), [0, 1]);

        candidates.push(("Select-queries.pdf", "Databases/Week 6/Select-queries.pdf"));
        assert_eq!(found(&candidates, "select"), [2]);
    }

    #[test]
    fn single_match_wins() {
        assert_eq!(single_choice(&["Lecture1.pdf"], "lec"), Some(0));
    }

    #[test]
    fn exact_name_wins_over_fuzzy_siblings() {
        let names = ["Lecture1-notes.pdf", "Lecture1.pdf", "lecture10.pdf"];
        assert_eq!(single_choice(&names, "lecture1.pdf"), Some(1));
        assert_eq!(single_choice(&names, "lecture1"), None);
    }

    #[test]
    fn duplicate_exact_names_still_ask() {
        let names = ["notes.pdf", "notes.pdf"];
        assert_eq!(single_choice(&names, "notes.pdf"), None);
    }
}
