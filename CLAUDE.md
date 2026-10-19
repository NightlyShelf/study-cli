# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Status: early implementation

Remote: github.com/NightlyShelf/study-cli (GPL-3.0). **`SPEC.md` is the source
of truth** — read it before doing anything; the sections below summarise what
matters for working here and point into it. When code and spec disagree, fix
one to match the other — don't let them drift. Milestone progress is tracked
in SPEC.md §10.

## What this is

`study` — a Rust CLI/TUI (binary name `study`) to download and upload files fast
from Moodle and Microsoft Teams channel files, read Moodle pages and Teams
channel messages, and show a Moodle calendar and task list. Not
university-specific: it targets any Moodle with the mobile web service enabled
and any Microsoft 365 tenant. The Moodle URL is configured, never hard-coded.

## Architecture (see SPEC.md §6, §9)

Planned Cargo workspace, four crates:

- `study-core` — domain types, the index (a JSON cache, not a DB), config, secrets
- `study-moodle` — Moodle REST client (web-service functions in SPEC §11)
- `study-graph` — Microsoft Graph client (device-code auth, drives, uploads)
- `study-cli` — clap subcommands + the ratatui TUI; the `study` binary

Load-bearing ideas:
- **The index is a local metadata snapshot** so `find` is instant/offline; the
  network is only touched to download/upload or to open the reader. `sync`
  refreshes it; commands auto-sync when it is stale (SPEC §6).
- **Auth reuses official clients** (SPEC §5). Both are settled — do not re-probe:
  - Moodle: mobile-app launch flow → token; works for any login method.
  - Teams: device-code with the **Microsoft Teams** first-party client
    (`1fec8e78-bce4-4aaf-ab1b-5451cc387264`) + the `.default` scope. Named Graph
    scopes on other first-party clients fail with `AADSTS65002`. **Do not cycle
    through client IDs probing for scopes** — it is settled and that pattern
    trips safety tooling.
- A reference implementation (Python/TS) for both APIs lives at
  github.com/shinzo13/uni (`backend/src/uni/sources/`); it grounds the function
  names and token decoding in SPEC §5 and §11.

## Code style — non-negotiable (SPEC §9.1)

The owner's explicit top priority is **simple, readable code they can read
top-to-bottom**. Enforce, and apply to your own contributions:

- No abstraction without two real callers *today*. The only sanctioned trait is
  `Source` (Moodle + Teams); everything else stays concrete.
- Prefer plain functions and structs over traits, generics, builders, newtypes.
  Concrete `async fn`s, no `Box<dyn>` soup.
- Short functions, early returns, shallow nesting, flat module tree.
- Flat errors: one `thiserror` enum per lib crate, `anyhow` in the binary.
- No macros beyond derive. Comments say *why*, not *what*.
- A change that adds indirection without removing more complexity than it adds
  is rejected.

## Commands (intended; from SPEC §9.2 — wired up in milestone 1)

```sh
cargo build
cargo test                              # all tests
cargo test <name>                       # a single test by name substring
cargo test -p study-moodle              # one crate's tests
cargo fmt                               # format (stable rustfmt, rustfmt.toml)
cargo fmt --check                       # format check (what the hook/CI runs)
cargo clippy --all-targets -- -D warnings
typos                                   # spell check
pre-commit run --all-files              # run every hook manually
```

Lints are centralised in `[workspace.lints]` in the root `Cargo.toml` — add or
change lints there, not with scattered `#![deny]` attributes. Do not enable
`clippy::pedantic`.

## Workflow: subagents by default

Implementation is delegated, not done inline. The main session coordinates:

1. Read the relevant `SPEC.md` sections and split the work into independent
   tasks (e.g. one crate, one command, one TUI tab).
2. Spawn one general-purpose subagent per task, **in parallel in a single
   message** when tasks are independent. Each brief includes: the exact SPEC
   sections to implement, the §9.1 code-style rules, and a concrete
   done-condition — compiles, `cargo clippy --all-targets -- -D warnings`
   clean, tests pass.
3. The main session verifies the result itself (fmt-check, clippy, test), fixes
   integration seams, and makes the commit (Conventional Commits, no scope).

Use the `Explore` agent for read-only codebase questions, a fresh
general-purpose agent for implementation, and a `fork` only when the task
needs this conversation's full context. Never let a subagent commit or push.

## Conventions

- **Commits:** Conventional Commits with a **bare type prefix and no scope** —
  `feat:`, `fix:`, `docs:`, `refactor:`, `chore:`; never `feat(cli):`. Commit
  only when asked; author commits as the repo owner's GitHub account.
- **Partial commits and `Cargo.lock`:** pre-commit stashes unstaged changes
  while hooks run, and marks a hook *Failed* if it modifies a tracked file.
  So a commit whose staged `Cargo.lock` doesn't match its staged manifests
  fails `cargo clippy` with "files were modified by this hook" even though
  clippy is clean. Commit one crate at a time with a lock that matches it
  (stash the other crates' manifest edits, `cargo build`, stage the lock),
  and always read the full hook output — never grep it down to one line.
- **CI:** one GitHub Actions workflow, triggered **only on PRs to `main`**,
  running fmt-check → clippy → test → typos. The pre-commit hooks are the
  primary gate; CI is the merge gate. Don't add per-push or per-branch CI.
- **README:** keep it short, in the style of github.com/NightlyShelf/captee —
  badges, one-liner, a short "unlike…" line, then Features / Install / Usage /
  Configuration. Examples over prose.
