# Changelog

Release notes are taken from the section headed `## [X.Y.Z]` matching the tag
`vX.Y.Z`: rename `Unreleased` to the version and date when releasing.

## [Unreleased]

- Claude Desktop: a profile's backup leaves out Chromium's caches at any
  depth (the preview browser keeps ~1 GB under `Partitions/`), and only the
  newest three backups of each kind are kept per profile
  (`CLAUDE_SWITCH_KEEP_BACKUPS`; `0` keeps them all). The first switch after
  updating removes that profile's older backups; one that holds the
  transcripts is never removed.
- A sync's backup lists the cards it adds instead of copying every card.
- Disk Space…: how much each profile, the backups, the shared VM and the
  transcripts take, and a clean-up of the caches, older Claude Code versions
  and old backups of the profiles not in use. Sessions, logins and
  transcripts stay.

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
