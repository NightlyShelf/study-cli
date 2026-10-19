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
    score: u32,
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
    download::download(&client, hits[chosen].file, &dir, false, false).await?;
    Ok(())
}

/// Matches against `course/section/file` so a course or week name narrows
/// the search too; best score first.
fn search<'a>(index: &'a Index, query: &str) -> Vec<Hit<'a>> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Ignore, Normalization::Smart);
    let mut buf = Vec::new();

    let mut hits = Vec::new();
    for file in &index.files {
        let location: Vec<&str> = path::ancestors(index, &file.node)
            .iter()
            .map(|node| node.name.as_str())
            .collect();
        let location = location.join("/");
        let haystack = format!("{location}/{}", file.name);
        let Some(score) = pattern.score(Utf32Str::new(&haystack, &mut buf), &mut matcher) else {
            continue;
        };
        hits.push(Hit {
            file,
            location,
            score,
        });
    }
    hits.sort_by_key(|hit| std::cmp::Reverse(hit.score));
    hits
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
