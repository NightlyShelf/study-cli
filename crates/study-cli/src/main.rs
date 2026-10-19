//! `study` — fast Moodle and Teams file access from the terminal.

mod alias;
mod app;
mod cal;
mod download;
mod find;
mod fmt;
mod get;
mod handler;
mod login;
mod ls;
mod path;
mod put;
mod sync;
mod teams_sync;
mod todo;
mod tui;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "study",
    version,
    about = "Download course files from Moodle fast"
)]
struct Cli {
    /// Without a subcommand, opens the interactive browser
    #[command(subcommand)]
    command: Option<Command>,
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
    /// Set a short alias for a course (`study ls <alias>`), or show it
    Alias {
        /// An alias, id or name prefix of the course
        course: String,
        /// The new alias; omitted, prints the current one
        alias: Option<String>,
    },
    /// Receives the moodlemobile:// redirect from the browser during `login`
    #[command(hide = true)]
    MoodleCallback { url: String },
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
    /// Show calendar events by day
    Cal {
        /// The next 7 days (the default)
        #[arg(long, conflicts_with = "month")]
        week: bool,
        /// The next 31 days
        #[arg(long)]
        month: bool,
    },
    /// List assignments by due date
    Todo {
        /// Include submitted, graded and long-past assignments
        #[arg(long)]
        all: bool,
    },
    /// Upload files to a Moodle assignment as a draft submission
    Put {
        #[arg(required = true)]
        files: Vec<PathBuf>,
        /// `<course>/<assignment>`; a course alias, id or name prefix,
        /// then the assignment title or a prefix of it
        #[arg(long)]
        to: String,
        /// Also submit for grading (usually irreversible; asks first)
        #[arg(long)]
        submit: bool,
        /// Skip the submit confirmation
        #[arg(long)]
        yes: bool,
    },
}

#[derive(Subcommand)]
enum LoginSource {
    /// Mobile-app login flow; works with any login method the site offers
    Moodle {
        /// Site root, e.g. https://moodle.example.edu (remembered afterwards)
        url: Option<String>,
    },
    /// Device-code login with the Microsoft Teams client
    Teams,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Some(command) = Cli::parse().command else {
        return tui::run().await;
    };
    match command {
        Command::Login {
            source: LoginSource::Moodle { url },
        } => login::moodle(url).await,
        Command::Login {
            source: LoginSource::Teams,
        } => login::teams().await,
        Command::Sync => sync::run().await,
        Command::Ls { reference } => ls::run(reference).await,
        Command::Find { query } => find::run(&query.join(" ")).await,
        Command::Alias { course, alias } => alias::run(&course, alias.as_deref()).await,
        Command::MoodleCallback { url } => handler::write_callback(&url),
        Command::Get {
            reference,
            dir,
            force,
            overwrite,
        } => get::run(&reference, dir, force, overwrite).await,
        Command::Cal { week: _, month } => cal::run(month).await,
        Command::Todo { all } => todo::run(all).await,
        Command::Put {
            files,
            to,
            submit,
            yes,
        } => put::run(&files, &to, submit, yes).await,
    }
}
