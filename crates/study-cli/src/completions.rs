//! `study completions`: print a shell completion script, or install it where
//! the shell loads it.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use clap_complete::Shell;

/// Shown by `study completions --help`; paths are the per-user defaults.
pub const HELP: &str = "\
Where the script goes:
  bash   study completions bash > ~/.local/share/bash-completion/completions/study
  zsh    study completions zsh > ~/.local/share/zsh/site-functions/_study
         then near the top of ~/.zshrc:  fpath=(~/.local/share/zsh/site-functions $fpath)
  fish   study completions fish > ~/.config/fish/completions/study.fish

`--install` writes the file there for you (bash, zsh, fish). For zsh it also
adds the setup to ~/.zshrc ($ZDOTDIR/.zshrc if set), once, in a block marked
`study completions`. Open a new shell afterwards.";

const ZSHRC_MARKER: &str = "# >>> study completions >>>";

pub fn run(shell: Shell, install: bool) -> Result<()> {
    let mut command = crate::Cli::command();
    if !install {
        clap_complete::generate(shell, &mut command, "study", &mut std::io::stdout());
        return Ok(());
    }

    let dirs = directories::BaseDirs::new().context("no home directory")?;
    let path = install_target(shell, dirs.data_dir(), dirs.config_dir())?;

    let dir = path.parent().context("completion path has no parent")?;
    fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut command, "study", &mut script);
    fs::write(&path, script).with_context(|| format!("writing {}", path.display()))?;
    println!("wrote {}", path.display());

    // zsh searches no user-writable directory by default, so the script alone
    // is never enough; bash and fish auto-load their per-user directories.
    if shell != Shell::Zsh {
        println!("open a new shell");
        return Ok(());
    }
    add_zshrc_setup(dir, dirs.home_dir())
}

/// Appends the fpath/compdef block to the user's zshrc unless it already has it.
fn add_zshrc_setup(dir: &Path, home: &Path) -> Result<()> {
    let rc_dir = match std::env::var_os("ZDOTDIR") {
        Some(zdotdir) if !zdotdir.is_empty() => PathBuf::from(zdotdir),
        _ => home.to_path_buf(),
    };
    let rc = rc_dir.join(".zshrc");

    let current = match fs::read_to_string(&rc) {
        Ok(text) => text,
        Err(err) if err.kind() == ErrorKind::NotFound => String::new(),
        Err(err) => return Err(err).with_context(|| format!("reading {}", rc.display())),
    };
    if zshrc_has_setup(&current, dir, home) {
        println!("{} already loads it; open a new shell", rc.display());
        return Ok(());
    }

    // Finish an unterminated last line so the blank line before the block is real.
    let lead = if current.is_empty() || current.ends_with('\n') {
        "\n"
    } else {
        "\n\n"
    };
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&rc)
        .with_context(|| format!("opening {}", rc.display()))?;
    write!(file, "{lead}{}", zshrc_block(dir))
        .with_context(|| format!("writing {}", rc.display()))?;
    println!(
        "added completion setup to {}; open a new shell",
        rc.display()
    );
    Ok(())
}

/// The file the shell loads completions from. Pure so the tests never touch
/// the real home.
fn install_target(shell: Shell, data_dir: &Path, config_dir: &Path) -> Result<PathBuf> {
    match shell {
        Shell::Bash => Ok(data_dir.join("bash-completion/completions/study")),
        Shell::Fish => Ok(config_dir.join("fish/completions/study.fish")),
        Shell::Zsh => Ok(data_dir.join("zsh/site-functions/_study")),
        // No per-user autoload directory to put it in.
        _ => bail!(
            "--install is not supported for {shell}; redirect the script to where your shell \
             loads it, e.g. study completions {shell} > <file>"
        ),
    }
}

/// The zshrc block for completions in `dir`. It works wherever it lands
/// relative to the user's own compinit: before it, compinit picks up the
/// fpath entry; after it, compdef registers `_study` directly; with no
/// compinit at all, the block starts it.
fn zshrc_block(dir: &Path) -> String {
    format!(
        "{ZSHRC_MARKER}\n\
         fpath=({} $fpath)\n\
         (( $+functions[compdef] )) || {{ autoload -Uz compinit && compinit; }}\n\
         autoload -Uz _study && compdef _study study\n\
         # <<< study completions <<<\n",
        dir.display()
    )
}

/// Whether the zshrc text already sets up completions from `dir`, either via
/// our block or a line the user wrote themselves (absolute or `~/` form).
fn zshrc_has_setup(rc: &str, dir: &Path, home: &Path) -> bool {
    if rc.contains(ZSHRC_MARKER) || rc.contains(&dir.display().to_string()) {
        return true;
    }
    match dir.strip_prefix(home) {
        Ok(rel) => rc.contains(&format!("~/{}", rel.display())),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/home/u";
    const DATA: &str = "/home/u/.local/share";
    const CONFIG: &str = "/home/u/.config";
    const SITE: &str = "/home/u/.local/share/zsh/site-functions";

    fn target(shell: Shell) -> PathBuf {
        install_target(shell, Path::new(DATA), Path::new(CONFIG)).unwrap()
    }

    fn has_setup(rc: &str) -> bool {
        zshrc_has_setup(rc, Path::new(SITE), Path::new(HOME))
    }

    #[test]
    fn bash_goes_under_data_dir() {
        assert_eq!(
            target(Shell::Bash),
            Path::new("/home/u/.local/share/bash-completion/completions/study")
        );
    }

    #[test]
    fn fish_goes_under_config_dir() {
        assert_eq!(
            target(Shell::Fish),
            Path::new("/home/u/.config/fish/completions/study.fish")
        );
    }

    #[test]
    fn zsh_goes_under_site_functions() {
        assert_eq!(
            target(Shell::Zsh),
            Path::new("/home/u/.local/share/zsh/site-functions/_study")
        );
    }

    #[test]
    fn other_shells_are_refused() {
        let err = install_target(Shell::Elvish, Path::new(DATA), Path::new(CONFIG))
            .unwrap_err()
            .to_string();
        assert!(err.contains("not supported for elvish"), "{err}");
        assert!(err.contains("study completions elvish >"), "{err}");
    }

    #[test]
    fn zshrc_block_extends_fpath_and_registers_compdef() {
        assert_eq!(
            zshrc_block(Path::new(SITE)),
            "# >>> study completions >>>\n\
             fpath=(/home/u/.local/share/zsh/site-functions $fpath)\n\
             (( $+functions[compdef] )) || { autoload -Uz compinit && compinit; }\n\
             autoload -Uz _study && compdef _study study\n\
             # <<< study completions <<<\n"
        );
    }

    #[test]
    fn zshrc_with_marker_has_setup() {
        let rc = format!(
            "export EDITOR=vim\n\n{}",
            zshrc_block(Path::new("/elsewhere"))
        );
        assert!(has_setup(&rc));
    }

    #[test]
    fn zshrc_with_absolute_dir_has_setup() {
        assert!(has_setup(
            "fpath=(/home/u/.local/share/zsh/site-functions $fpath)\ncompinit\n"
        ));
    }

    #[test]
    fn zshrc_with_tilde_dir_has_setup() {
        assert!(has_setup(
            "fpath=(~/.local/share/zsh/site-functions $fpath)\ncompinit\n"
        ));
    }

    #[test]
    fn plain_zshrc_lacks_setup() {
        assert!(!has_setup(
            "fpath=(/usr/share/zsh/site-functions $fpath)\nautoload -Uz compinit && compinit\n"
        ));
        assert!(!has_setup(""));
    }
}
