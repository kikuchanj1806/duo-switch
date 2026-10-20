#!/bin/sh
# Build what a macOS release ships, into dist/, without GitHub Actions:
#   sh scripts/build-release-macos.sh            # Apple Silicon and Intel
#   sh scripts/build-release-macos.sh arm64      # one of them
#
# For each architecture: the .dmg people install from, the .app.tar.gz the
# updater installs (signed afterwards by scripts/publish-release.sh), the
# command line tool, and a .sha256 beside each. Nothing is signed or uploaded
# here, so this needs no key and no password.
set -eu

[ "$(uname -s)" = "Darwin" ] || { echo "Run this on a Mac."; exit 1; }
cd "$(dirname "$0")/.."
command -v cargo >/dev/null 2>&1 || PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"

version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
arches=${*:-arm64 x64}
rm -rf dist && mkdir dist

for arch in $arches; do
  case "$arch" in
    arm64) target=aarch64-apple-darwin ;;
    x64) target=x86_64-apple-darwin ;;
    *) echo "unknown architecture: $arch (use arm64 or x64)"; exit 1 ;;
  esac
  echo "== $arch ($target), version $version"
  rustup target add "$target" >/dev/null
  cargo build --release -p pitstop-app -p pitstop-cli --target "$target"
  dir="target/$target/release"
  sh crates/pitstop-app/macos/bundle.sh "$dir/pitstop-app" >/dev/null

  # The disk image people install from, with the guide beside the app.
  stage=$(mktemp -d)
  cp -R "$dir/Duo Switch.app" "$stage/"
  ln -s /Applications "$stage/Applications"
  cp HUONG-DAN-CLAUDE-DESKTOP.md "$stage/"
  hdiutil create -quiet -volname "Duo Switch" -srcfolder "$stage" -ov -format UDZO \
    "dist/DuoSwitch-macos-$arch.dmg"
  rm -rf "$stage"

  # What the updater installs: the app bundle alone, no ._ files.
  COPYFILE_DISABLE=1 tar czf "dist/DuoSwitch-macos-$arch.app.tar.gz" -C "$dir" "Duo Switch.app"

  cli="duo-switch-cli-macos-$arch"
  mkdir "dist/$cli"
  cp "$dir/duo-switch" README.md LICENSE "dist/$cli/"
  tar czf "dist/$cli.tar.gz" -C dist "$cli"
  rm -rf "dist/$cli"
done

cd dist
for f in *; do
  shasum -a 256 "$f" >"$f.sha256"
done
ls -la
echo
echo "Built $version into dist/. Next: sh scripts/publish-release.sh"
