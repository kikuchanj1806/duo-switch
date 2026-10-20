#!/bin/sh
# Build "Duo Switch.app" + .dmg for Apple Silicon. Run on a Mac, from the repo root:
#   sh build-macos-arm64.sh
set -eu

[ "$(uname -s)" = "Darwin" ] || { echo "Run this on a Mac."; exit 1; }
[ "$(uname -m)" = "arm64" ]  || { echo "This script targets Apple Silicon (arm64)."; exit 1; }

# Xcode command line tools (linker, codesign, hdiutil)
xcode-select -p >/dev/null 2>&1 || { echo "Run: xcode-select --install"; exit 1; }

# Rust toolchain
if ! command -v cargo >/dev/null 2>&1; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  . "$HOME/.cargo/env"
fi
rustup target add aarch64-apple-darwin

cargo build --release -p pitstop-app -p pitstop-cli --target aarch64-apple-darwin
dir=target/aarch64-apple-darwin/release

sh crates/pitstop-app/macos/bundle.sh "$dir/pitstop-app"

rm -rf dmg dist && mkdir -p dmg dist
cp -R "$dir/Duo Switch.app" dmg/
ln -s /Applications dmg/Applications
cp HUONG-DAN-CLAUDE-DESKTOP.md dmg/
hdiutil create -volname "Duo Switch" -srcfolder dmg -ov -format UDZO dist/DuoSwitch-macos-arm64.dmg
rm -rf dmg

echo
echo "App: $dir/Duo Switch.app"
echo "DMG: dist/DuoSwitch-macos-arm64.dmg"
echo "CLI: $dir/duo-switch"
echo "First open: System Settings > Privacy & Security > Open Anyway"
echo "(or: xattr -dr com.apple.quarantine \"/Applications/Duo Switch.app\")"
