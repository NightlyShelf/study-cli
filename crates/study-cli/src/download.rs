//! Streaming download with the collision rules of SPEC §7.

use std::ffi::OsString;
use std::fs::{self, Metadata};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use indicatif::{ProgressBar, ProgressStyle};
use study_core::index::File;

/// Streams `response` (from `app::fetch`) into `dir` as `file`, returning
/// where it ended up. An unchanged local copy is skipped unless `force`; an
/// existing file gets a `name (2).ext` sibling unless `overwrite`. `quiet`
/// silences stdout and the progress bar, which would corrupt the TUI's
/// alternate screen.
pub async fn download(
    response: reqwest::Response,
    file: &File,
    dir: &Path,
    force: bool,
    overwrite: bool,
    quiet: bool,
) -> Result<PathBuf> {
    let mut dest = dir.join(&file.name);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    if dest.exists() && !overwrite {
        if !force && is_unchanged(&fs::metadata(&dest)?, file) {
            if !quiet {
                println!("skip (unchanged): {}", file.name);
            }
            return Ok(dest);
        }
        dest = free_name(dir, &file.name);
    }

    let bar = if quiet {
        ProgressBar::hidden()
    } else {
        progress_bar(&file.name, response.content_length())
    };
    let part = part_path(&dest);
    let written = write_stream(response, &part, &bar).await;
    bar.finish_and_clear();
    if written.is_err() {
        // Never leave a truncated `.part` lying around.
        let _ = fs::remove_file(&part);
    }
    written.with_context(|| format!("downloading {}", file.name))?;
    fs::rename(&part, &dest).with_context(|| format!("renaming to {}", dest.display()))?;

    // Mirror the remote mtime so the unchanged check works next time.
    if let Some(mtime) = file.mtime {
        let handle = fs::OpenOptions::new().write(true).open(&dest)?;
        handle.set_modified(UNIX_EPOCH + Duration::from_secs(mtime))?;
    }
    if !quiet {
        println!("{}", dest.display());
    }
    Ok(dest)
}

/// True when the local copy has the same size and modification time as the
/// remote one; the sizes being unknown counts as changed.
pub fn is_unchanged(meta: &Metadata, file: &File) -> bool {
    let local_mtime = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|age| age.as_secs());
    file.size == Some(meta.len()) && file.mtime.is_some() && file.mtime == local_mtime
}

/// First of `name`, `stem (2).ext`, `stem (3).ext`, … that does not exist
/// under `dir`. `name` may contain subdirectories.
pub fn free_name(dir: &Path, name: &str) -> PathBuf {
    let name = Path::new(name);
    let folder = dir.join(name.parent().unwrap_or(Path::new("")));
    let stem = name.file_stem().unwrap_or_default().to_string_lossy();
    let extension = name
        .extension()
        .map(|ext| format!(".{}", ext.to_string_lossy()))
        .unwrap_or_default();

    let mut candidate = folder.join(name.file_name().unwrap_or_default());
    let mut n = 2;
    while candidate.exists() {
        candidate = folder.join(format!("{stem} ({n}){extension}"));
        n += 1;
    }
    candidate
}

fn part_path(dest: &Path) -> PathBuf {
    let mut part: OsString = dest.as_os_str().to_owned();
    part.push(".part");
    PathBuf::from(part)
}

fn progress_bar(name: &str, length: Option<u64>) -> ProgressBar {
    let bar = match length {
        Some(length) => ProgressBar::new(length),
        None => ProgressBar::no_length(),
    };
    let style = ProgressStyle::default_bar()
        .template("{msg} {bytes}/{total_bytes} {bar:30} {bytes_per_sec}")
        .expect("template is valid");
    bar.set_style(style);
    bar.set_message(name.to_string());
    bar
}

async fn write_stream(response: reqwest::Response, part: &Path, bar: &ProgressBar) -> Result<()> {
    let mut out = fs::File::create(part)?;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        out.write_all(&chunk)?;
        bar.inc(chunk.len() as u64);
    }
    out.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn remote(name: &str, size: u64, mtime: u64) -> File {
        File {
            id: "f".to_string(),
            node: "n".to_string(),
            name: name.to_string(),
            size: Some(size),
            mtime: Some(mtime),
            mime: None,
            remote_id: "f".to_string(),
            download_url: "https://moodle.test/f".to_string(),
            web_url: None,
        }
    }

    #[test]
    fn free_name_counts_up() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(free_name(dir.path(), "a.pdf"), dir.path().join("a.pdf"));
        fs::write(dir.path().join("a.pdf"), b"x").unwrap();
        assert_eq!(free_name(dir.path(), "a.pdf"), dir.path().join("a (2).pdf"));
        fs::write(dir.path().join("a (2).pdf"), b"x").unwrap();
        assert_eq!(free_name(dir.path(), "a.pdf"), dir.path().join("a (3).pdf"));
    }

    #[test]
    fn free_name_without_extension_and_in_subdir() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("README"), b"x").unwrap();
        assert_eq!(
            free_name(dir.path(), "README"),
            dir.path().join("README (2)")
        );

        fs::create_dir(dir.path().join("sub")).unwrap();
        fs::write(dir.path().join("sub/notes.pdf"), b"x").unwrap();
        assert_eq!(
            free_name(dir.path(), "sub/notes.pdf"),
            dir.path().join("sub").join("notes (2).pdf")
        );
    }

    #[test]
    fn unchanged_needs_matching_size_and_mtime() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pdf");
        fs::write(&path, b"hello").unwrap();
        let handle = fs::OpenOptions::new().write(true).open(&path).unwrap();
        handle
            .set_modified(UNIX_EPOCH + Duration::from_secs(1_700_000_000))
            .unwrap();
        let meta = fs::metadata(&path).unwrap();

        assert!(is_unchanged(&meta, &remote("a.pdf", 5, 1_700_000_000)));
        assert!(!is_unchanged(&meta, &remote("a.pdf", 6, 1_700_000_000)));
        assert!(!is_unchanged(&meta, &remote("a.pdf", 5, 1_700_000_001)));

        let mut unknown = remote("a.pdf", 5, 0);
        unknown.mtime = None;
        assert!(!is_unchanged(&meta, &unknown));
    }

    #[test]
    fn part_path_appends_suffix() {
        assert_eq!(
            part_path(Path::new("/x/a.pdf")),
            PathBuf::from("/x/a.pdf.part")
        );
    }
}
