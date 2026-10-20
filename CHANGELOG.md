# Changelog

Release notes are taken from the section headed `## [X.Y.Z]` matching the tag
`vX.Y.Z`: rename `Unreleased` to the version and date when releasing.

## [0.5.1] - 2026-10-20

- Claude Desktop: each profile shows its five-hour and weekly limits, read
  from the usage Claude Desktop records itself, with estimated reset times
  and pace; running out suggests (and notifies) a switch to a profile with
  room. Assigning an account picks the one signed in and asks for its email.

## [0.5.0] - 2026-10-20

- Claude Desktop: switch between profiles (one account each), assign each its
  account, bring Code Local sessions along by hand or on every switch
  (auto-sync), with backups before every change. Shares `~/ClaudeProfiles`
  with `claude-switch`.
- Codex Desktop: switching an account quits the app first and opens it again
  on the new login.
- Removed Claude Code CLI account switching, warm-ups and the automatic switch.
- Vietnamese guide for people the app is shared with: `HUONG-DAN-CLAUDE-DESKTOP.md`.
