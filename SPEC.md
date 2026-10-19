# study — Moodle + Teams file CLI/TUI

Status: DRAFT v0.1 (2026-10-19). Open questions are marked `[?]`.

## 1. Goal

Get a file out of Moodle or Microsoft Teams (or into them) in seconds, without
clicking through course pages, sections, folders and channel tabs. Secondary:
read Moodle pages and Teams channel messages, and see Moodle calendar events
and upcoming tasks — all from the same tool.

**Not university-specific.** It targets any Moodle with the mobile web service
enabled and any Microsoft 365 tenant that issues a token to the Teams client
(§5). Developed and tested against one AMU setup; expected to work elsewhere
but untested there. The Moodle URL is configured, never hard-coded.

Non-goals (v1): 1:1/group chat, grades, Teams assignments, chat attachments,
full offline mirror, Windows/macOS packaging polish.

## 2. User decisions (locked)

| Topic             | Decision                                                        |
|-------------------|-----------------------------------------------------------------|
| Language          | Rust                                                            |
| Interface         | Full TUI (ratatui), plus scriptable subcommands                 |
| Keys              | Vim-style; arrows also work                                     |
| Moodle site       | any Moodle w/ mobile service (dev site: `lms.amu.edu.pl/sci`)    |
| Teams scope       | Channel files only (SharePoint document libraries)              |
| Teams auth        | No own Azure app registration possible                          |
| Download target   | Current working directory, on demand                            |
| Moodle upload     | Upload as draft by default; `--submit` finalises                |

## 3. Commands

Binary name: `study`. Running it with no arguments starts the TUI.

```
study                          TUI
study login moodle             mobile-app SSO token flow (section 5.1)
study login teams              device-code flow (section 5.2)
study whoami                   show both accounts and token health
study sync [--moodle|--teams]  refresh local index (incremental when possible)
study ls [<course>[/<path>]]   list courses / folder contents from index
study find <query...>          fuzzy search the index; single exact match downloads, else pick
study get <course>/<path>      download to cwd (or -o <dir>); tab-completable
study put <file> --to <target> upload (section 7)
study cal [--week|--month]     Moodle calendar
study todo                     action events (due assignments, quizzes…)
study open <course>/<path>     print the web URL (or open with xdg-open)
study completions <shell>
```

Conventions:
- Output is a plain table when stdout is a terminal, TSV when piped.
  `--json` everywhere.
- Course references accept the Moodle short name, a case-insensitive prefix,
  or `teams:<team>/<channel>`. Ambiguity → list candidates and exit 2.
- Existing file in cwd: skip if size and mtime match the remote file,
  otherwise write `name (2).ext`. `--force` re-downloads, `--overwrite`
  replaces.

## 4. TUI

Three panes, three tabs. Everything is driven from the local index, so the
UI never blocks on the network except for downloads/uploads and opening the
reader (§4.3).

```
┌ Files │ Calendar │ Todo ─────────────────────────────────────────────┐
│ Sources        │ algo / Week 5                       │ Lecture5.pdf  │
│ > algo         │   Lecture5_Graphs.pdf     2.1M      │ 2.1 MB        │
│   os           │   hw5.pdf                  120K     │ modified 3d   │
│   db           │ 💬 Messages (General)                │ resource      │
│   ml  (teams)  │ 📄 Syllabus (page)                   │ ↵ download    │
│                │ > seminar/                           │ Databases 24/SZ│
├────────────────┴──────────────────────────────────────┴───────────────┤
│ /graph_                                   12 matches   synced 4 min  │
└───────────────────────────────────────────────────────────────────────┘
```

Sources (left) lists Moodle courses and Teams teams together, each by its
alias (§4.1). Middle lists the selected course's contents. Right shows
metadata for the focused item, including the real (un-aliased) course name.

### 4.1 Course aliases

Course names are long; the TUI shows a short alias instead. Press `a` on a
course → type an alias → persisted to `config.toml` under `[aliases]`, keyed
by `<source>:<remote_id>` (stable across renames) with the real name as a
trailing comment. Aliases are also valid `<course>` references on the CLI
(`study get algo/Week 5/Lecture5.pdf`). Un-aliased courses show their real
name, truncated.

### 4.2 Enter is polymorphic

| Focused item            | Enter does                                      |
|-------------------------|-------------------------------------------------|
| file                    | download to the launch directory                |
| folder                  | expand; `Enter` again on it downloads recursively |
| Moodle `page` / `label` | open reader with its rendered HTML (§4.3)        |
| Teams `💬 Messages`      | open reader with the channel transcript (§4.3)   |
| `url` link              | `o` opens in browser (not downloadable)          |

Teams layout: a team with only a **General** channel shows its files directly
under the course (the channel is hidden) plus a `💬 Messages (General)` entry.
A team with extra channels shows those as folders, each with its own messages
entry.

### 4.3 Reader overlay

One shared scrollable overlay renders HTML as text (`html2text`), used for
both Moodle pages/labels and Teams channel messages. Content is fetched **on
demand** when the reader opens (brief spinner), not stored in the index —
messages are volatile and pages are cheap — and cached for the session.

- Moodle page/label: `mod_page_get_pages_by_courses` for the course → the
  module's HTML; labels use the module description.
- Teams messages: `GET /teams/{id}/channels/{cid}/messages?$expand=replies&$top=50`,
  rendered as a transcript (author · time, body, replies indented).

Reader keys: `j k` / arrows scroll, `g g`/`G` ends, link markers `[1] [2]…`
with `o <n>` to open a link in the browser, `Esc` closes back to the tree.

### 4.4 Keys

| Key          | Action                                              |
|--------------|-----------------------------------------------------|
| h j k l / ←↓↑→ | move / collapse / expand                           |
| Enter        | act on item (§4.2)                                  |
| a            | set/change alias for the focused course             |
| /            | filter current pane; `/` on Sources filters globally|
| Tab / S-Tab  | next / previous pane; `1` `2` `3` switch tabs       |
| u            | upload: pick local file (path prompt) to focused target |
| o            | open in browser                                     |
| y            | copy web URL to clipboard                           |
| g g / G      | top / bottom                                        |
| r            | sync (background; status in footer)                 |
| ?            | key help                                            |
| q / Esc      | close reader / back / quit                          |

Downloads go to the directory `study` was started from. Footer shows progress
for active transfers; errors are shown in the footer and in `study whoami`.

Calendar tab: month grid on the left, selected day's events on the right.
Todo tab: coursework sorted by due date; Enter opens the assignment's files in
the Files tab; `u` uploads a submission.

## 5. Authentication

Auth is the critical path. Both flows reuse official clients, so they depend
on server-side settings the tool can detect but not control (§5 caveats).

### 5.1 Moodle (mobile-app flow)

Works for any Moodle with the mobile web service enabled, whatever the login
method (password, SSO/CAS, OAuth) — the browser handles login, so the tool
needs no per-site auth code. `study login moodle <url>` (prompted if omitted;
stored in config) drives:

0. Fetch `tool_mobile_get_public_config` (public, no token) to confirm the
   mobile service is enabled and read the real `launchurl`; fail early with a
   clear message if it is disabled.
1. Generate a random `passport` and open the `launchurl`:
   `…/admin/tool/mobile/launch.php?service=moodle_mobile_app&passport=<p>&urlscheme=moodlemobile`
2. User authenticates via SSO. Moodle redirects to
   `moodlemobile://token=<base64>` (the `urlscheme` parameter may be ignored
   by `forcedurlscheme`; the page also shows a fallback link).
3. The CLI obtains that URL either from a registered `x-scheme-handler`
   (`study moodle-callback <url>`, Linux .desktop file, optional) or by the
   user pasting it into the waiting `study login moodle` prompt.
4. Decode base64 → split on `:::` into `signature:::token:::privatetoken`.
   Verify `signature == md5(wwwroot + passport)` (stop if mismatched); keep
   `token` (privatetoken unused). Then call `core_webservice_get_site_info`
   to record `userid`, site name, and the list of available functions.

Web-service calls: POST to `<site>/webservice/rest/server.php` with
`wstoken`, `wsfunction`, `moodlewsrestformat=json`, and params flattened as
`name[key][0]=…`. A response that is a dict with an `exception` field is an
error; `errorcode` in {`invalidtoken`,`accessexception_token_expired`} means
the token is dead → prompt re-login. (Flow and decoding confirmed against the
reference impl, `backend/src/uni/sources/moodle/client.py`.)

Dev-site check (2026-10-19, `lms.amu.edu.pl/sci`): `enablemobilewebservice=1`,
`typeoflogin=2` (browser-based), IdP "CAS UAM". Any site reporting
`enablemobilewebservice=1` is expected to work the same way.

Token storage: OS keyring via `keyring` crate; fallback to
`$XDG_CONFIG_HOME/study/secrets.toml` with mode 0600.

### 5.2 Teams / SharePoint (Microsoft Graph, device-code)

No own app registration is possible. The CLI reuses the **Microsoft Teams**
first-party public client with the OAuth device-code flow and the `.default`
scope, which returns exactly the Graph permissions that client is already
preauthorized for (files, sites, channel messages) — no named-scope consent
is requested, so the tenant's user-consent policy is not involved.

```
client_id = 1fec8e78-bce4-4aaf-ab1b-5451cc387264   # Microsoft Teams, overridable
authority = https://login.microsoftonline.com/organizations/oauth2/v2.0
scope     = https://graph.microsoft.com/.default offline_access
```

Flow: POST `/devicecode` → show `user_code` + `verification_uri` → poll
`/token` with `grant_type=device_code` until a `refresh_token` comes back
(treat `authorization_pending`/`slow_down` as "keep polling"). Store the
refresh token (§5 keyring). Access tokens are fetched per scope from the
refresh token, cached with a ~120 s expiry margin, and refreshed on 401;
honour refresh-token rotation. `invalid_grant`/`interaction_required` mean
the grant is dead → prompt re-login.

Why this works where the earlier spike failed: the Azure CLI client
(`04b07795-...`) with *named* scopes `Files.ReadWrite.All`/`Sites.ReadWrite.All`
returns `AADSTS65002` because that client is not preauthorized for them.
Switching to the Teams client + `.default` sidesteps named-scope
preauthorization entirely. Approach confirmed by the reference implementation
at github.com/shinzo13/uni (`backend/src/uni/sources/teams/`).

Honest caveats (stated in the README):
- Relies on undocumented first-party preauthorized scopes; Microsoft can
  change or revoke this at any time. Not a stable contract.
- Entra sign-in logs attribute activity to "Microsoft Teams", not this tool.
  It is the user's own account and data, but the user accepts this grey area.
- Conditional-access / device-compliance policies can still reject the token;
  no workaround, and the CLI surfaces the AADSTS code verbatim.

Education **Assignments** (not in v1 scope — channel files only) would need a
second token on the `https://onenote.com/.default` scope against
`assignments.onenote.com/api/v1.0/edu`. Noted for a later milestone.

Refresh token stored like the Moodle token. Access tokens cached in memory
and refreshed on 401.

## 6. Data model and index

**Why an index exists:** listing every file live costs ~11 round-trips for
Moodle (2–5 s) and dozens-to-hundreds for Teams (10–30 s+). Paying that on
every `find` would defeat "fast". The index is a local metadata snapshot so
search is instant and offline; the network is touched only to download. The
fuzzy matcher (nucleo) loads candidates into memory at search time regardless
of backend, so persistence is purely "don't re-crawl every run". Deleting the
index is always safe.

**Storage: a JSON cache**, not a database. At student scale (≈10–50 courses,
a few thousand files) a single `index.json` at `$XDG_DATA_HOME/study/` loads
and filters in < 50 ms with no native dependency. SQLite (`rusqlite`) is a
documented upgrade path if data grows large or background-sync/TUI concurrency
demands it; the data model below is storage-agnostic.

```
source  { kind: moodle|teams, name, remote_id, last_sync, delta_token? }
node    { id, source, parent?, kind: course|section|module|folder|team|channel,
          modname?, name, remote_id, web_url }   // the container tree
file    { id, node, name, size, mtime, mime, remote_id, download_url, web_url }
event   { id, source, kind: calendar|coursework, title, start, due?,
          course_id?, cmid?, status?, grade?, url }
```

Readable content (Moodle page/label HTML, Teams messages) is **not** stored
here — the index holds only the `node` pointing at it (`modname="page"`, or a
channel node). The reader (§4.3) fetches the body on demand and caches it for
the session. Aliases live in `config.toml`, not the index, so wiping the index
never loses them.

**When sync runs:** `study sync` refreshes explicitly; other commands
auto-sync when the cache is missing or `last_sync` is older than ~15 min (tunable),
so manual `sync` is rarely needed. `--force` ignores the freshness check.

Sync:
- Moodle: `core_webservice_get_site_info` (→ `userid`) →
  `core_enrol_get_users_courses` (`userid`) → per course
  `core_course_get_contents` (`courseid`). Walk sections → modules, skipping
  any with `uservisible == false`. For each module, the `contents[]` entries
  with `type == "file"` become `file` rows (`filename`, `fileurl`,
  `filesize`, `mimetype`, `timemodified`); `modname` maps to module kind
  (`resource`/`folder` → files, `url`/`page` → link recorded, not
  downloadable). Module mtime = max file `timemodified`, else
  `contentsinfo.lastmodified`. This mtime feeds both incremental sync and the
  skip-if-identical download check. Full sync of ~10 courses < 5 s.
- Teams: first sync does a one-time deep crawl — `me/joinedTeams` →
  `teams/{id}/channels` → `channels/{id}/filesFolder` →
  `drives/{d}/items/{id}/children` recursively (the slow 10–30 s). It stores
  a `delta` token per drive; every later sync calls `drives/{d}/root/delta`
  and applies only the changes, so Teams stays fully indexed but cheap to
  refresh. This keeps global `find` complete *and* fast after the first run.
  A team with a single (General) channel is flattened: its channel node is
  dropped and its files/messages attach directly to the course node (§4.2).
  Teams with extra channels keep per-channel folder nodes.

## 7. Transfers

Download:
- Moodle: `fileurl` + `token=<token>` (append with `&` or `?` as needed;
  the URL is a `webservice/pluginfile.php` link). Streamed to `<cwd>/<name>`
  via a `.part` file, then renamed. Collision: skip if size+mtime match,
  else `name (2).ext` unless `--overwrite`.
- Teams: `GET /drives/{d}/items/{id}/content` (follows 302 to
  pre-authenticated URL). Folders are downloaded recursively into
  `<cwd>/<folder>/`.

Upload (`study put <file> --to <target>`):
- Moodle assignment (`--to <course>/<assignment>`): resolve the assignment
  via `mod_assign_get_assignments` (match name/prefix within the course;
  ambiguity → list + exit 2). Upload each file with `core_files_upload`,
  reusing the returned `itemid` as the draft area for subsequent files, then
  `mod_assign_save_submission` with
  `plugindata[files_filemanager]=<itemid>`. `--submit` then calls
  `mod_assign_submit_for_grading` (irreversible on many sites; warns and
  requires `--yes` when non-interactive).
  Caveat: these three functions must be in the mobile service's allowed list.
  The official app submits assignments, so they normally are; `login` records
  the available-functions list and `put` fails early with a clear message if
  one is missing. The reference impl is read-only for Moodle, so upload is
  **unverified** and gets its own check in milestone 3.
- Teams channel folder (`--to teams:<team>/<channel>[/<path>]`):
  `PUT /drives/{d}/items/{parent}:/{name}:/content` for files ≤ 4 MB,
  otherwise `createUploadSession` with 5 MiB chunks and resume on failure.
- Progress bar in CLI, footer progress in TUI.

## 8. Calendar and todo

- `study cal` (Calendar tab): `core_calendar_get_calendar_events` for the
  range, rendered day-grouped; `--month` renders a grid. These are the raw
  events you asked for.
- `study todo` (Todo tab): built from coursework, not calendar events, which
  the reference impl shows is more reliable and yields submission state for
  free. Per enrolled course: `mod_assign_get_assignments` (→ title, `duedate`,
  `allowsubmissionsfromdate`, `cmid`, `introattachments`) joined with
  `mod_assign_get_submission_status` (→ `new`/`draft`/`submitted`, plus
  graded). Optionally fold in quizzes via `mod_quiz_get_quizzes_by_courses`
  (`timeclose` as due). Sorted by due date, grouped by course, relative due
  times, submitted/graded markers. Enter opens the assignment's
  `introattachments` in the Files tab; `u` uploads a submission.
- Both read from the index; `sync` refreshes events and coursework alongside
  files. Submission status cached ~10 min.

## 9. Architecture

```
crates/
  study-core      domain types, index (JSON cache), config, secrets
  study-moodle    REST client (serde models for the WS functions above)
  study-graph     Graph client (device-code auth, drives, upload sessions)
  study-cli       clap commands + TUI (ratatui + crossterm), the `study` binary
```

Key crates: tokio, reqwest (rustls), serde/serde_json (JSON index), clap,
ratatui, crossterm, keyring, nucleo (fuzzy matching), html2text (reader),
indicatif, directories, tracing. (SQLite/`rusqlite` only if the index outgrows
JSON.) Errors: `thiserror` in libs, `anyhow` in the binary.

Config: `$XDG_CONFIG_HOME/study/config.toml`:

```toml
[moodle]
url = ""   # set by `study login moodle <url>`; e.g. https://moodle.your-uni.edu
[teams]
client_id = "1fec8e78-bce4-4aaf-ab1b-5451cc387264"   # Microsoft Teams first-party
authority = "https://login.microsoftonline.com/organizations"
[ui]
hidden_courses = []

[aliases]
# "<source>:<remote_id>" = "alias"   # managed by the TUI `a` key
"moodle:4821" = "algo"               # Algorithms and Data Structures 2024/SZ
"teams:19:abc…@thread.tacv2" = "ml"  # 2024/SZ Machine Learning (lab)
```

### 9.1 Code style — simple and readable above all

The overriding goal is code the author can read top-to-bottom without
decoding abstractions. Concrete rules, in priority order:

- **No abstraction without two real callers.** Add a trait, generic, or
  helper only when there is a second concrete use *today*, not a hypothetical
  one. The one sanctioned trait is `Source` (Moodle + Teams both implement
  it); everything else stays concrete until duplication forces otherwise.
- **Prefer plain functions and structs** over traits, generics, builders, and
  newtype wrappers. Pass concrete types; return concrete types.
- **No async-trait object soup.** Each client exposes concrete `async fn`s.
  No `Box<dyn Trait>` unless a collection genuinely holds mixed types.
- **Short functions, early returns, shallow nesting.** If a function needs a
  comment to explain its *sections*, split it. Flat module tree; small files.
- **Errors are flat.** One `thiserror` enum per crate for library errors;
  `anyhow` in the binary. No layered error-conversion hierarchies.
- **No macros beyond derive.** No DSLs, no clever generics, no type-level
  tricks.
- **Comments say why, not what**, at the density of the surrounding code.
- When two approaches work, pick the one a newcomer reads faster, even if it
  is a few lines longer.

This is a stated review criterion: a change that adds indirection without
removing more complexity than it introduces is rejected.

### 9.2 Tooling

- **Format:** `cargo fmt` with stable rustfmt and a minimal `rustfmt.toml`
  (`edition = "2021"`, default style otherwise — no nightly-only options, so
  `cargo fmt --check` works on stable in CI and the hook).
- **Lint:** `cargo clippy --all-targets -- -D warnings`. Lints are centralised
  in a `[workspace.lints]` table in the root `Cargo.toml` (so no scattered
  `#![deny]` attributes): `clippy::all` at warn level plus
  `unsafe_code = "forbid"`; the `-D warnings` on the hook/CI command line is
  what turns warnings into failures, so a plain `cargo build` still works
  mid-edit. **Not** `clippy::pedantic` — it pushes toward noise that fights
  readability. Add individual pedantic lints only if they prove useful.
- **Checks:** `cargo check --all-targets`, `cargo test`, and `typos` for
  spelling.
- **Pre-commit:** a `.pre-commit-config.yaml` (the pre-commit framework) with
  local hooks, fast ones first so failures surface early:
  1. `cargo fmt --check`
  2. `cargo clippy --all-targets -- -D warnings`
  3. `cargo test`
  4. `typos`
  (Lefthook is a drop-in alternative if a Python-free hook runner is
  preferred; same four steps.) The hooks are the primary gate.
- **CI:** one lightweight GitHub Actions workflow, triggered **only on pull
  requests targeting `main`** (not on every push/branch), running the same
  four steps. The pre-commit hooks catch everything locally; CI is just the
  merge gate.
- **Commits:** Conventional Commits, author = the repo owner's GitHub account.
  Use a bare type prefix — `feat:`, `fix:`, `docs:`, `refactor:`, `chore:` —
  **without** a scope (`feat:`, never `feat(cli):`).

### 9.3 README

Short and concise, in the style of github.com/NightlyShelf/captee: a CI badge
and a Rust-version badge, a one-line description, a short "unlike…"
differentiator, then `## Features`, `## Install`, `## Usage` (a handful of
`study` command examples + the two `login` flows), `## Configuration` (config
and index paths). No wall of prose; examples over explanation.

## 10. Milestones

| # | Deliverable | Go/no-go |
|---|-------------|----------|
| 0 | Auth: (a) Moodle mobile service confirmed enabled on the real site ✓; (b) Teams device-code with Teams client + `.default` confirmed viable via reference impl ✓ | both resolved |
| 1 | Workspace scaffold: four crates compile, `rustfmt.toml`, `[workspace.lints]`, `.pre-commit-config.yaml`, PR-to-main CI, short README (§9.2–9.3) | done 2026-10-19 (`27f2134`) |
| 2 | `login moodle`, `sync`, `ls`, `find`, `get` for Moodle | |
| 3 | TUI Files tab (Moodle), aliases, reader overlay for pages | |
| 4 | `put` for Moodle assignments; Todo tab | |
| 5 | Calendar tab, `cal` | |
| 6 | Teams: `login teams`, sync, get/put, messages reader, TUI integration | |
| 7 | Shell completions, incremental sync, polish | |

## 11. API reference (grounded in the reference impl)

Moodle web-service functions (all via `webservice/rest/server.php`):

| Purpose              | Function                                         |
|----------------------|--------------------------------------------------|
| site info / userid   | `core_webservice_get_site_info`                  |
| enrolled courses     | `core_enrol_get_users_courses`                   |
| course files/modules | `core_course_get_contents`                       |
| assignments          | `mod_assign_get_assignments`                     |
| submission status    | `mod_assign_get_submission_status`               |
| quizzes (optional)   | `mod_quiz_get_quizzes_by_courses`                |
| calendar events      | `core_calendar_get_calendar_events`              |
| page content (reader)| `mod_page_get_pages_by_courses`                  |
| file upload (draft)  | `core_files_upload`                              |
| save submission      | `mod_assign_save_submission`                     |
| submit for grading   | `mod_assign_submit_for_grading`                  |

File download: the `fileurl` from `core_course_get_contents`/
`introattachments`, with `token=<token>` appended.

Microsoft Graph (v1.0) endpoints:

| Purpose              | Endpoint                                         |
|----------------------|--------------------------------------------------|
| teams                | `GET /me/joinedTeams`                            |
| channels             | `GET /teams/{id}/channels`                        |
| channel files folder | `GET /teams/{id}/channels/{cid}/filesFolder`      |
| channel messages     | `GET /teams/{id}/channels/{cid}/messages?$expand=replies` |
| folder children      | `GET /drives/{d}/items/{id}/children`             |
| incremental          | `GET /drives/{d}/root/delta`                       |
| download             | `GET /drives/{d}/items/{id}/content` (302)        |
| small upload         | `PUT /drives/{d}/items/{p}:/{name}:/content`      |
| large upload         | `POST …/createUploadSession` + 5 MiB chunks       |

## 12. Open questions

- `[?]` Does AMU conditional access permit the Teams-client token on an
  unmanaged Linux device? Only a real `login teams` run will tell; surfaces
  as an AADSTS code if not.
