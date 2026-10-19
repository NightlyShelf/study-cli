use std::path::PathBuf;

use directories::ProjectDirs;

/// `~/.config/study` on Linux; `STUDY_CONFIG_DIR` overrides it.
pub fn config_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("STUDY_CONFIG_DIR") {
        return PathBuf::from(dir);
    }
    match ProjectDirs::from("", "", "study") {
        Some(dirs) => dirs.config_dir().to_path_buf(),
        // No home directory at all; fall back to the working directory.
        None => PathBuf::from("study"),
    }
}

/// `~/.local/share/study` on Linux; `STUDY_DATA_DIR` overrides it.
pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("STUDY_DATA_DIR") {
        return PathBuf::from(dir);
    }
    match ProjectDirs::from("", "", "study") {
        Some(dirs) => dirs.data_dir().to_path_buf(),
        None => PathBuf::from("study"),
    }
}
