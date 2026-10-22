# AGENTS.md

Guidance for AI coding agents working on rettui, a Reticulum client with a
terminal UI (ratatui) and a web UI (axum + plain JS), written in Rust.
Read `wiki/Development.md` for the code layout and the CI and release workflows.

## Branches, commits and pull requests

- Work on `dev`. Pull requests go from `dev` to `main`.
- A large feature may get its own branch off `dev`, named
  `feature/<name>`, and merge back into `dev` when it's done. Before
  starting a very large feature request, ask whether it should have its
  own branch and how to split it up. Don't push to any other branch unless
  asked.
- Don't open a pull request unless asked.
- **Always use [Conventional Commits](https://www.conventionalcommits.org/):**
  `type(scope): summary`, imperative mood, no trailing period. Use one of
  these types: `feat`, `fix`, `docs`, `test`, `refactor`, `perf`, `build`,
  `ci`, `chore`. The scope is the area touched, for example `config`,
  `identities`, `voice`, `web`, `tui`, `deps`. Mark a breaking change with `!` and a
  `BREAKING CHANGE:` footer. The body says what changed and why.
- Never rewrite pushed history (no force-push, rebase or amend on `dev`).
  To bring in another branch, merge it.
- Never put AI model names or identifiers in commits, code, comments, docs
  or anything else committed.

## Before every push

Run these and make sure they pass. CI runs the same checks on Linux,
macOS and Windows:

    cargo fmt                                            # rustfmt.toml: 140 columns, wide style
    cargo clippy --locked --all-targets -- -D warnings
    cargo test --locked
    cargo deny check                                     # if installed

- Coverage (`cargo llvm-cov`) must stay at or above 85% of lines. Codecov
  fails a change that drops coverage by more than 3%. New code comes with
  tests.
- Code that only runs on Windows or macOS still has to compile there. When
  you touch it, run `cargo check --all-targets --target x86_64-pc-windows-gnu`.
- When CI fails, find and fix the root cause. Never skip, disable or
  quarantine a test. "Flaky" is not a diagnosis: find the race or timing
  assumption and fix it.

## Features

- A user-facing feature goes in **both** the TUI and the web UI, unless one
  of them can't safely have it (see Security).
- New settings need all of these:
  - a field in `Settings` and its `Default`;
  - an entry in `SECTIONS` and in `FIELDS` (with label and help text);
  - `field_value` and `set_field`;
  - `Field::web_access` if the web UI mustn't change it;
  - a value in the `settings_that_apply_now_do` test.
- TUI keys go in `src/ui/keys.rs`, which feeds the footer and the `?` popup,
  and in `wiki/Using-the-TUI.md`.
- Follow other clients' formats (Sideband, Columba, MeshChat, NomadNet) so
  rettui interoperates with them. Sideband and LXST are under non-commercial
  or no-derivatives licences: read their formats, but never copy their code.

## Security

- Anyone holding the web UI login link controls the web UI. So the web UI
  must never:
  - run programs or choose what runs (pipe interface commands, executable
    pages, the voice recorder command);
  - read, export, import or delete identity keys.

  Make those settings `TerminalOnly`, and keep those actions TUI-only.
- Treat everything that comes from the network (LXMF fields, Micron pages,
  RRC, game moves) as untrusted. Parse it without panicking, and cap sizes
  and lengths.
- Web jobs run in the owner loop, so a panic there takes the whole web UI
  down. Never call `unwrap` on input in handlers or jobs.

## Files and data

- Write files with `config::write_atomic` or `config::write_private`. Both
  call `sync_all` and then `config::replace`, which retries the rename on
  Windows (virus scanners and the indexer hold new files open briefly).
  Don't use a bare `fs::rename` to replace a saved file.
- Changes to saved data must stay readable both ways. Add new fields with
  `#[serde(default)]`, skipped when empty, so older versions still read the
  files. Never rename or remove a field that is already saved.
- Per-identity data lives in `Paths::base`. Shared data (log, web login and
  certificates, caches, Reticulum config history) lives in `Paths::root`.

## Dependencies

- `deps/` holds git submodules. Only change one whose remote is a fork
  under the repository owner's account (`zevaryx`): check `.gitmodules`
  for each submodule's current remote before touching it. Commit a change
  in the fork, then move the submodule pointer in rettui. Leave every
  other submodule as it is, and work around its limits in rettui itself.
- `deps/rsReticulum` is pinned on purpose. Never change it or move its
  submodule pointer, even though it's a fork, unless the open PRs that it
  is forked from are merged into upstream.
- Never open a pull request against an upstream project for a change made
  in a fork.
- `vendor/codec2` is a vendored copy of the codec2 crate, with 700C, 450 and
  450PWB added. Record every change to it in `vendor/codec2/README.md`
  (LGPL-2.1).
- Don't link system libraries that servers and Docker images don't have,
  such as ALSA. Call an installed program instead, as voice recording does.
- When cargo-deny reports an advisory, update the crate
  (`cargo update -p <crate>`) and add a Security entry to the changelog.
  New licences must be allowed in `deny.toml`.
- If the Dockerfile has to build something new, copy it in there
  (`deps/`, `vendor/`, `src/`).

## Code style

- Match the surrounding code: its naming, idioms and comment density.
- Comments are plain English. They say why, or what something is, briefly,
  in the same voice as the existing code. Don't restate the code.
- User-facing text (help, notices, docs) is plain and concrete. Say what
  happens and why, without jargon.
- Tests drive the app the way a person would. Use `ui::tui_tests::Tui` for
  keys and clicks, `web::api_tests` with `TestWeb` for the API, and
  `test_app` / `test_app_with_net` for app logic. Give each test its own
  temp directory.

## Docs and changelog

- User docs live in `wiki/`. CI publishes them to the GitHub wiki and the
  docs site, so never edit the GitHub wiki directly. Update the pages a
  change affects in the same commit.
- Every user-visible change gets an entry in `CHANGELOG.md`, under the
  unreleased version's section: **Added**, **Fixed**, **Security** or
  **Docs and development**. Write it for users, and link to the wiki page.
  Keep that section's opening summary line up to date.
- Versioning: bump the major version only for real breaks in user-facing
  compatibility (data directory, settings, CLI, web API). New features
  bump the minor version. The version in `Cargo.toml` and the changelog
  heading must match the release tag.
