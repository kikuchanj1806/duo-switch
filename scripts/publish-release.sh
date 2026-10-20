#!/bin/sh
# Sign the updates in dist/ and publish them as a GitHub release, without
# GitHub Actions. Run after scripts/build-release-macos.sh, in a terminal:
#   sh scripts/publish-release.sh
#
# Asks for the updater key's password; the key stays in ~/.tauri and the
# password is never written down. Publishes tag vX.Y.Z (Cargo.toml's
# version) with the CHANGELOG.md section "## [X.Y.Z]" as notes, and the
# latest.json the installed apps read.
set -eu

cd "$(dirname "$0")/.."
key=${TAURI_SIGNING_PRIVATE_KEY_PATH:-$HOME/.tauri/duo-switch.key}
repo=$(gh repo view --json nameWithOwner -q .nameWithOwner)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
tag="v$version"

[ -f "$key" ] || { echo "No updater key at $key"; exit 1; }
ls dist/*.app.tar.gz >/dev/null 2>&1 || { echo "Nothing to publish: run scripts/build-release-macos.sh first"; exit 1; }
git rev-parse -q --verify "refs/tags/$tag" >/dev/null || { echo "Tag $tag does not exist yet"; exit 1; }
# Without the tag on GitHub, gh would tag whatever main is now.
git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null || { echo "Tag $tag is not on GitHub yet: git push origin $tag"; exit 1; }
if gh release view "$tag" >/dev/null 2>&1; then
  echo "Release $tag already exists on GitHub"; exit 1
fi

# The section of CHANGELOG.md for this version.
notes=$(mktemp)
awk -v v="$version" '
  $0 ~ "^## \\[" v "\\]" { found = 1; next }
  found && /^## \[/ { exit }
  found { print }
' CHANGELOG.md >"$notes"
[ -s "$notes" ] || { echo "CHANGELOG.md has no section \"## [$version]\""; exit 1; }

printf "Password of the updater key %s: " "$key"
trap 'stty echo' EXIT INT TERM
stty -echo
read -r password
stty echo
echo

# The password goes by environment, never on a command line others can list.
export TAURI_SIGNING_PRIVATE_KEY_PASSWORD="$password"
base="https://github.com/$repo/releases/download/$tag"
platforms='{}'
for pair in darwin-aarch64=DuoSwitch-macos-arm64.app.tar.gz \
            darwin-x86_64=DuoSwitch-macos-x64.app.tar.gz; do
  target=${pair%%=*}
  file=${pair#*=}
  [ -f "dist/$file" ] || continue
  # Bound to its version: the app refuses a newer number paired with an
  # older signed file.
  npx --yes @tauri-apps/cli@2 signer sign -f "$key" --app-version "$version" "dist/$file"
  platforms=$(printf '%s' "$platforms" | jq --arg t "$target" --arg url "$base/$file" \
    --rawfile sig "dist/$file.sig" '. + {($t): {url: $url, signature: $sig}}')
done
unset TAURI_SIGNING_PRIVATE_KEY_PASSWORD password

jq -n --arg version "$version" --rawfile notes "$notes" \
  --arg date "$(date -u +%Y-%m-%dT%H:%M:%SZ)" --argjson platforms "$platforms" \
  '{version: $version, notes: ($notes | sub("^\\s+"; "") | sub("\\s+$"; "")),
    pub_date: $date, platforms: $platforms}' >dist/latest.json
cat dist/latest.json

{
  echo
  echo "## Download"
  echo
  echo "| System | File |"
  echo "|---|---|"
  echo "| macOS 11+, Apple Silicon | \`DuoSwitch-macos-arm64.dmg\` |"
  echo "| macOS 11+, Intel | \`DuoSwitch-macos-x64.dmg\` |"
  echo
  echo "The app is not signed by Apple. First open: **System Settings → Privacy & Security → Open Anyway**, or run \`xattr -dr com.apple.quarantine \"/Applications/Duo Switch.app\"\`. Step by step (Vietnamese): https://github.com/$repo/blob/main/HUONG-DAN-CLAUDE-DESKTOP.md"
  echo
  echo "Each download has a \`.sha256\` checksum next to it. The \`.sig\` files and \`latest.json\` are for the app's own updater."
} >>"$notes"

# Only what a release ships: no logs or other strays left in dist/.
gh release create "$tag" --title "Duo Switch $tag" --notes-file "$notes" \
  dist/*.dmg dist/*.tar.gz dist/*.sig dist/*.sha256 dist/latest.json
rm -f "$notes"
echo
echo "Published: https://github.com/$repo/releases/tag/$tag"
