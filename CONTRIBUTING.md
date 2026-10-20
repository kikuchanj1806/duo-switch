# Contributing

Thanks for helping. Bug reports, ideas and pull requests are welcome — in English
or Vietnamese.

By contributing you agree that your contribution is licensed under this project's
[license](LICENSE) (MIT with the Commons Clause).

## Reporting a bug

Open an [issue](https://github.com/kikuchanj1806/duo-switch/issues/new/choose) with your
OS, the app version (Settings), which tool (Claude Desktop / Codex) and what you
expected. **Never paste tokens, `.credentials.json`, `auth.json` or anything from
`~/.pitstop/vault/`.** Security problems go through [SECURITY.md](SECURITY.md)
instead.

## Development

Layout:

| Crate | What |
|---|---|
| `crates/pitstop-core` | accounts, vault, switching, usage, the Usage pane's log reader |
| `crates/pitstop-cli` | the `duo-switch` command line |
| `crates/pitstop-app` | the Tauri 2 app (tray + window); UI in `ui/` |

```sh
cargo build -p pitstop-app
PITSTOP_HOME=/tmp/sandbox cargo run -p pitstop-app   # against a sandbox, not your real accounts
```

The window's CSS is Tailwind v4: edit `crates/pitstop-app/ui-src/app.css`, then run
`npm install && npm run build` in `crates/pitstop-app/ui-src`; commit the built
`ui/app.css` too. Dark-theme rules use `:root[data-theme="dark"]`.

Before opening a pull request, run what CI runs:

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/)
(`feat(app): …`, `fix(core): …`), and user-visible changes get a line in
[CHANGELOG.md](CHANGELOG.md).

## Releasing

Without GitHub Actions, from a Mac with the updater key in `~/.tauri/duo-switch.key`:
bump `version`, date its `CHANGELOG.md` section, commit, tag `vX.Y.Z` and push the
tag, then run `sh scripts/build-release-macos.sh` and `sh scripts/publish-release.sh`
(it asks for the key's password, signs, writes `latest.json` and publishes).

With GitHub Actions:

Bump `version` in the root `Cargo.toml` (the app takes its version from there),
date the section in `CHANGELOG.md`, then push a tag `vX.Y.Z`. The Release workflow
builds the Windows installer and portable zip, the macOS `.dmg`s, and publishes
them with the changelog section as notes.

It also signs what the updater installs (the Windows setup and a
`DuoSwitch-macos-*.app.tar.gz` per Mac) and publishes `latest.json`, which the
apps already out there read from `releases/latest/download/latest.json`. Signing
needs two repository secrets, `TAURI_SIGNING_PRIVATE_KEY` (the key file's content)
and `TAURI_SIGNING_PRIVATE_KEY_PASSWORD`. The matching public key is
`plugins.updater.pubkey` in `crates/pitstop-app/tauri.conf.json`. Keep the private
key backed up: without it no installed app accepts another update, and a new key
means everyone downloads the next version by hand. Without the secrets the release
still builds, with a warning, but offers no update.

## macOS checklist (first run on a Mac)

- [ ] `cargo test --workspace` — includes Unix-only file-permission tests
- [ ] `Duo Switch.app` appears in the menu bar with no Dock icon; the Dock icon
      shows only while its window is open
- [ ] Choosing a Codex account swaps `~/.codex/auth.json`; a running Codex
      Desktop is asked to quit first and opened again after
- [ ] Claude Desktop: switching a profile quits Claude, relinks
      `~/Library/Application Support/Claude`, and opens it again
- [ ] **Add Account** for Codex: the browser sign-in works and the new account is
      parked without signing out the one in use
- [ ] **Open Duo Switch at login** adds a LaunchAgent and starts the app in the menu bar
