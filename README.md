# study

[![checks](https://github.com/NightlyShelf/study-cli/actions/workflows/checks.yml/badge.svg?branch=main)](https://github.com/NightlyShelf/study-cli/actions/workflows/checks.yml)
![Rust 1.99.1](https://img.shields.io/badge/Rust-1.99.1-dea584)

study is a CLI/TUI that gets files out of Moodle and Microsoft Teams (or into them) in seconds, and reads course pages, channel messages, calendar and tasks from the same terminal.

Unlike the web UIs, there is no clicking through course pages, sections, folders and channel tabs: fuzzy-find a file and get it. A local index makes search instant and offline; the network is touched only to download or upload.

## Features
- Fuzzy-find any file across all courses and teams and download it to the current directory
- Upload to Moodle assignments (draft by default, `--submit` to finalise) and Teams channel folders
- Read Moodle pages and Teams channel messages in the TUI reader
- Moodle calendar and a todo list of due assignments and quizzes
- Short course aliases (`algo` instead of "Algorithms and Data Structures 2024/SZ"), usable on the CLI too
- Works with any Moodle that has the mobile web service enabled and any Microsoft 365 tenant

## Install

From source for now:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh   # Rust, if needed
git clone https://github.com/NightlyShelf/study-cli
cargo install --path study-cli/crates/study-cli                  # installs `study`
```

## Usage

```sh
study login moodle https://moodle.your-uni.edu     # sign in in the browser, paste the moodlemobile:// link back
study login teams                                  # device-code flow with the Microsoft Teams client
study find lecture 5                               # fuzzy search; a single match downloads, otherwise pick
study get "algo/Week 5/Lecture5.pdf"               # download to cwd
study put hw5.pdf --to "algo/Homework 5" --submit  # upload and submit for grading
study todo                                         # due assignments and quizzes
study                                              # TUI
```

The TUI is vim-keyed (arrows work too); press `?` for the key help.

## Configuration

Config lives at `$XDG_CONFIG_HOME/study/config.toml` (usually `~/.config/study/`); the index cache at `$XDG_DATA_HOME/study/index.json` (usually `~/.local/share/study/`) can be deleted at any time.
The Moodle URL is set by `study login moodle <url>`; the Teams login reuses the Microsoft Teams client and needs no app registration.

## Notes

- Teams auth relies on an undocumented behaviour of the first-party Microsoft Teams client; Microsoft can change or revoke it at any time.
- Entra sign-in logs attribute the activity to "Microsoft Teams", and tenant conditional-access or device-compliance policies can still reject the token (the AADSTS code is shown verbatim).
- Developed and tested against one university; expected to work elsewhere but untested there.
