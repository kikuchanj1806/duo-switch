# Project notes for Claude Code

Tray/menu-bar app (Rust + Tauri, web UI) that switches between Claude Code and
Codex CLI accounts. This is a rebranded fork; the product name is whatever is in
`crates/pitstop-app/tauri.conf.json` (`productName`). Placeholder values
(owner, links, bundle id) were set by `rebrand.sh` and may still need real values.

## Layout
- `crates/pitstop-core/` – core logic: vault (AES-256-GCM), engine, providers, usage
- `crates/pitstop-app/src/` – Tauri app: tray, commands, settings, warm-up, updater
- `crates/pitstop-app/ui/` – web UI (plain JS/HTML). `app.css` is COMPILED – edit `ui-src/app.css`, then `cd crates/pitstop-app/ui-src && npm install && npm run build`
- `crates/pitstop-app/macos/bundle.sh` – builds the `.app` bundle
- `crates/pitstop-cli/` – command-line tool
- Internal names still say `pitstop` (crates, `~/.pitstop`, `PITSTOP_HOME`); not user-visible.

## Commands
- Test: `cargo test --workspace` (the rebrand has NOT been compiled yet – run this first and fix anything that fails)
- Run safely: `PITSTOP_HOME=/tmp/sandbox cargo run -p pitstop-app`
- Package for Apple Silicon: `sh build-macos-arm64.sh`

## Rules
- Do not use the real `~/.pitstop`, Keychain items or real accounts while testing; use `PITSTOP_HOME`.
- CSP is strict (`script-src 'self'`, `style-src 'self'`): no inline scripts/styles, no CDN; ship assets as files in `ui/`.
- Auto-update is off by default; the updater `pubkey` in tauri.conf.json belongs to the original author and cannot sign this fork's releases.
- Leave `LICENSE` as it is (MIT + Commons Clause).
