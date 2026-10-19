//! `study` — fast Moodle (and later Teams) file access from the terminal.

mod app;
mod download;
mod find;
mod fmt;
mod get;
mod login;
mod ls;
mod path;
mod sync;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "study",
    version,
    about = "Download course files from Moodle fast"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Log in to a source and store its token
    Login {
        #[command(subcommand)]
        source: LoginSource,
    },
    /// Refresh the local index from the server
    Sync,
    /// List courses, or the contents of <course>/<section>/...
    Ls {
        /// `<course>[/<segment>/...]`; an alias, id or name prefix
        reference: Option<String>,
    },
    /// Fuzzy-search every indexed file; a single exact match downloads
    Find {
        #[arg(required = true)]
        query: Vec<String>,
    },
    /// Download a file, or every file under a section/folder
    Get {
        /// `<course>/<segment>/...` ending in a file, section or folder
        reference: String,
        /// Directory to download into (default: current directory)
        #[arg(short = 'o', long = "out")]
        dir: Option<PathBuf>,
        /// Re-download even when the local copy looks unchanged
        #[arg(long)]
        force: bool,
        /// Replace an existing file instead of writing `name (2).ext`
        #[arg(long)]
        overwrite: bool,
    },
}

#[derive(Subcommand)]
enum LoginSource {
    /// Mobile-app login flow; works with any login method the site offers
    Moodle {
        /// Site root, e.g. https://moodle.example.edu (remembered afterwards)
        url: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    match Cli::parse().command {
        Command::Login {
            source: LoginSource::Moodle { url },
        } => login::moodle(url).await,
        Command::Sync => sync::run().await,
        Command::Ls { reference } => ls::run(reference).await,
        Command::Find { query } => find::run(&query.join(" ")).await,
        Command::Get {
            reference,
            dir,
            force,
            overwrite,
        } => get::run(&reference, dir, force, overwrite).await,
    }
}
