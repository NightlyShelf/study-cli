//! The `moodlemobile://` scheme handler: the browser runs
//! `study moodle-callback <url>`, which drops the redirect into a file that
//! the waiting `study login moodle` picks up (SPEC §5.1 step 3).

use std::fs;
use std::path::PathBuf;

use anyhow::Result;

/// Where the redirect url waits for the login command.
pub fn callback_path() -> PathBuf {
    study_core::paths::data_dir().join("moodle-callback")
}

/// `study moodle-callback <URL>`: the browser invokes this, so it must be
/// instant and silent. The url carries a token, hence the private mode.
pub fn write_callback(url: &str) -> Result<()> {
    let path = callback_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_private(&path, url)?;
    Ok(())
}

#[cfg(unix)]
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let mut file = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    file.write_all(contents.as_bytes())
}

#[cfg(not(unix))]
fn write_private(path: &std::path::Path, contents: &str) -> std::io::Result<()> {
    fs::write(path, contents)
}

/// Registers this binary as the `moodlemobile://` handler through a
/// `.desktop` file. Rewritten every time: the exe path differs between
/// `cargo run` and an installed binary.
#[cfg(target_os = "linux")]
pub fn install_handler() -> Result<()> {
    use std::process::Command;

    use anyhow::{Context, bail};

    let Some(dirs) = directories::BaseDirs::new() else {
        bail!("no home directory");
    };
    let exe = std::env::current_exe().context("locating the study binary")?;
    let entry = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=study (Moodle login)\n\
         NoDisplay=true\n\
         Exec=\"{}\" moodle-callback %u\n\
         MimeType=x-scheme-handler/moodlemobile;\n",
        exe.display()
    );
    let dir = dirs.data_dir().join("applications");
    fs::create_dir_all(&dir)?;
    fs::write(dir.join("study-moodle.desktop"), entry)?;

    let status = Command::new("xdg-mime")
        .args([
            "default",
            "study-moodle.desktop",
            "x-scheme-handler/moodlemobile",
        ])
        .status();
    match status {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => bail!("xdg-mime failed ({status})"),
        // Without xdg-mime the browser cannot hand the login back, but
        // pasting the link still works.
        Err(err) => {
            eprintln!("warning: could not run xdg-mime ({err}); paste the link instead");
            Ok(())
        }
    }
}

#[cfg(not(target_os = "linux"))]
pub fn install_handler() -> Result<()> {
    Ok(())
}
