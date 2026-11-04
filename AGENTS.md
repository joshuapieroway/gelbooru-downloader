# AGENTS.md — Gelbooru Downloader

Rust 2024 edition async CLI/TUI downloader. Working dir: /opt/gelbooru-downloader.

## Build / Test

- `cargo build` (dev), `cargo build --release`
- `cargo test` (unit + integration)
- `cargo clippy` (optional)
- Binary name: `gelbooru` (target/debug/gelbooru)

## Architecture

Modules under src/: cli, api, downloader, database, filters, presets, batch, watch, repl, tui, config, utils.

- config: TOML + env overrides, XDG dirs.
- api: reqwest client, models, search pagination.
- filters: client-side FilterRules.
- downloader: DownloadEngine with semaphore concurrency, streaming to .part, atomic rename, blake3 hash, validation.
- database: rusqlite history, presets, watch state.
- tui: ratatui.

## Conventions

- Errors: utils::error::AppError / Result.
- Async DB via spawn_blocking.
- No placeholders; keep tests passing.
- Never commit secrets; API creds via env/CLI/config.

## Repository maintenance

Keep this repository deliberately simple: one branch, one binary, no extra
process.

- Work directly on `main`. Do not create feature, release, or long-lived
  branches unless the user explicitly asks for one.
- Push in coherent batches: after a feature or fix is complete and all checks
  pass. Do not push partial work or every small edit.
- Before pushing:
  - `cargo fmt --check`
  - `cargo test`
  - `cargo build --release`
- Never force-push `main`.
- Keep the tree small. `/target`, databases, downloads, preview caches, and
  local config are ignored; never commit generated artifacts or secrets.
- Follow the release workflow below for version bumps, tags, and GitHub
  releases.
- Do not add CI, extra tooling, or new documentation files unless they solve a
  concrete problem; extend the existing files instead.
- Commit messages: short imperative summary, e.g. `Add viewer quick download`.

## Changelog

`CHANGELOG.md` is the only changelog and the source of GitHub release notes.
Use [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) with
[Semantic Versioning](https://semver.org/spec/v2.0.0.html). Newest version first.

- One section per released version: `## [X.Y.Z] - YYYY-MM-DD`.
- Directly below the heading, add a compare link against the previous tag, as
  shown in the skeleton below.
- Group entries under `### Added`, `### Changed`, `### Fixed` in that order.
  Add `### Removed`, `### Deprecated`, `### Security`, or `### Documentation`
  only when they apply.
- Compare against the previous tag and cover the complete diff, not just the
  latest commit. Review `git log vOLD..HEAD` and `git diff vOLD..HEAD` first.
- Describe user-visible behaviour: commands, keys, paths, defaults, and error
  messages in backticks. Skip internal refactors unless they change behaviour
  or performance.
- One change per bullet; keep bullets to 1-3 lines and wrap at 80 columns.
- Use the release date, not the commit date.
- Never rewrite or reorder sections for versions that are already tagged.
- Keep the `0.1.0` section as the initial-release baseline.

Skeleton:

```markdown
## [X.Y.Z] - YYYY-MM-DD

Compare: [`vOLD...vNEW`](https://github.com/joshuapieroway/gelbooru-downloader-rs/compare/vOLD...vNEW)

One or two sentences summarising the release.

### Added

- ...

### Changed

- ...

### Fixed

- ...
```

## Releases and GitHub

- Remote: `origin` is
  `https://github.com/joshuapieroway/gelbooru-downloader-rs.git`.
- GitHub account: `joshuapieroway`. Verify `gh` access with `gh auth status`
  before releasing.
- Work on `main` directly; do not create release or feature branches. Never
  force-push.
- Tags use `vX.Y.Z` (annotated). GitHub release titles use `X.Y.Z` without the
  `v` prefix.
- Create GitHub releases for meaningful versions only, not docs-only or
  check-only updates.
- Run all checks from "Repository maintenance" before tagging.
- Never move or delete a published tag. If a release is wrong, ship a new
  patch version.

Release steps:

1. Bump `version` in `Cargo.toml`, then run `cargo check` so `Cargo.lock` picks
   up the new version.
2. Update `CHANGELOG.md` using the format above.
3. Stage and commit the release:

   ```bash
   git add Cargo.toml Cargo.lock CHANGELOG.md <changed files>
   git commit -m "Release vX.Y.Z: <short summary>"
   ```

4. Push `main`, then tag and push the tag:

   ```bash
   git push origin main
   git tag -a vX.Y.Z -m "Version X.Y.Z: <short summary>"
   git push origin vX.Y.Z
   ```

5. Create the GitHub release from the matching changelog section:

   ```bash
   version=X.Y.Z
   awk -v v="$version" \
     'index($0, "## [" v "]") == 1 {on=1; next} on && /^## \[/ {on=0} on' \
     CHANGELOG.md > "/tmp/opencode/release-$version.md"
   gh release create "v$version" --title "$version" \
     --notes-file "/tmp/opencode/release-$version.md"
   ```

6. Verify the result:

   ```bash
   git status
   git log --oneline -1 origin/main
   gh release view "v$version"
   gh release list
   ```

## Notes

- Network available for cargo; offline cache present.
- Tests use wiremock.
