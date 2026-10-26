//! `study completions`: print a shell completion script, or install it where
//! the shell loads it.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use clap_complete::Shell;

/// Shown by `study completions --help`; paths are the per-user defaults.
pub const HELP: &str = "\
Where the script goes:
  bash   study completions bash > ~/.local/share/bash-completion/completions/study
  zsh    study completions zsh > ~/.local/share/zsh/site-functions/_study
         then in ~/.zshrc, before compinit:  fpath=(~/.local/share/zsh/site-functions $fpath)
  fish   study completions fish > ~/.config/fish/completions/study.fish

`--install` writes the file there for you (bash, zsh, fish) and prints the
line to add to your rc file, if any. Open a new shell afterwards.";

pub fn run(shell: Shell, install: bool) -> Result<()> {
    let mut command = crate::Cli::command();
    if !install {
        clap_complete::generate(shell, &mut command, "study", &mut std::io::stdout());
        return Ok(());
    }

    let dirs = directories::BaseDirs::new().context("no home directory")?;
    let fpath = std::env::var_os("FPATH");
    let (path, next_step) =
        install_target(shell, dirs.data_dir(), dirs.config_dir(), fpath.as_deref())?;

    let dir = path.parent().context("completion path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut command, "study", &mut script);
    fs::write(&path, script).with_context(|| format!("writing {}", path.display()))?;

    println!("wrote {}", path.display());
    println!("{next_step}");
    Ok(())
}

/// The file the shell loads completions from and what the user must do once
/// it is written. Pure so the tests never touch the real home.
fn install_target(
    shell: Shell,
    data_dir: &Path,
    config_dir: &Path,
    fpath: Option<&OsStr>,
) -> Result<(PathBuf, String)> {
    let new_shell = "open a new shell".to_string();
    match shell {
        Shell::Bash => Ok((
            data_dir.join("bash-completion/completions/study"),
            new_shell,
        )),
        Shell::Fish => Ok((config_dir.join("fish/completions/study.fish"), new_shell)),
        Shell::Zsh => {
            let dir = data_dir.join("zsh/site-functions");
            let on_fpath =
                fpath.is_some_and(|fpath| std::env::split_paths(fpath).any(|p| p == dir));
            let next_step = if on_fpath {
                new_shell
            } else {
                format!(
                    "add to ~/.zshrc before compinit:  fpath=({} $fpath)",
                    dir.display()
                )
            };
            Ok((dir.join("_study"), next_step))
        }
        // No per-user autoload directory to put it in.
        _ => bail!(
            "--install is not supported for {shell}; redirect the script to where your shell \
             loads it, e.g. study completions {shell} > <file>"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DATA: &str = "/home/u/.local/share";
    const CONFIG: &str = "/home/u/.config";

    fn target(shell: Shell, fpath: Option<&str>) -> (PathBuf, String) {
        install_target(
            shell,
            Path::new(DATA),
            Path::new(CONFIG),
            fpath.map(OsStr::new),
        )
        .unwrap()
    }

    #[test]
    fn bash_goes_under_data_dir() {
        let (path, next) = target(Shell::Bash, None);
        assert_eq!(
            path,
            Path::new("/home/u/.local/share/bash-completion/completions/study")
        );
        assert_eq!(next, "open a new shell");
    }

    #[test]
    fn fish_goes_under_config_dir() {
        let (path, next) = target(Shell::Fish, None);
        assert_eq!(
            path,
            Path::new("/home/u/.config/fish/completions/study.fish")
        );
        assert_eq!(next, "open a new shell");
    }

    #[test]
    fn zsh_tells_how_to_extend_fpath() {
        let (path, next) = target(Shell::Zsh, Some("/usr/share/zsh/site-functions"));
        assert_eq!(
            path,
            Path::new("/home/u/.local/share/zsh/site-functions/_study")
        );
        assert_eq!(
            next,
            "add to ~/.zshrc before compinit:  fpath=(/home/u/.local/share/zsh/site-functions $fpath)"
        );
    }

    #[test]
    fn zsh_skips_the_fpath_line_when_already_on_it() {
        let fpath = "/usr/share/zsh/site-functions:/home/u/.local/share/zsh/site-functions";
        let (_, next) = target(Shell::Zsh, Some(fpath));
        assert_eq!(next, "open a new shell");
    }

    #[test]
    fn other_shells_are_refused() {
        let err = install_target(Shell::Elvish, Path::new(DATA), Path::new(CONFIG), None)
            .unwrap_err()
            .to_string();
        assert!(err.contains("not supported for elvish"), "{err}");
        assert!(err.contains("study completions elvish >"), "{err}");
    }
}
