use std::fs;
use std::path::{Path, PathBuf};

use serde_json::json;

use super::control::fake::FakeApp;
use super::*;

struct World {
    _dir: tempfile::TempDir,
    home: PathBuf,
    d: Desktop<FakeApp>,
}

/// A home with Claude Desktop's own folder (signed in, with caches) and
/// Claude Code's history folder.
fn world_with(app: FakeApp) -> World {
    world_keeping(app, KEEP_BACKUPS)
}

fn world_keeping(app: FakeApp, keep: usize) -> World {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().to_path_buf();
    let app_data = home.join("Library/Application Support/Claude");
    fs::create_dir_all(app_data.join("Cache")).unwrap();
    fs::write(app_data.join("Cache/blob"), "cache").unwrap();
    fs::write(app_data.join("Cookies"), "signed in as A").unwrap();
    fs::create_dir_all(home.join(".claude/projects/-Users-me-app")).unwrap();
    let d = Desktop::new(Paths::new(&home, true), app)
        .unwrap()
        .keeping(keep);
    World { _dir: dir, home, d }
}

fn world() -> World {
    world_with(FakeApp::default())
}

impl World {
    fn root(&self) -> PathBuf {
        self.d.paths().root.clone()
    }
    fn calls(&self) -> Vec<&'static str> {
        self.d.app.calls.borrow().clone()
    }
    fn sessions(&self, profile: &str, pair: &str) -> PathBuf {
        let dir = self.root().join(profile).join(SESSIONS).join(pair);
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn card(&self, profile: &str, pair: &str, file: &str, value: serde_json::Value) {
        fs::write(
            self.sessions(profile, pair).join(file),
            card::to_bytes(&value),
        )
        .unwrap();
    }
    fn transcript(&self, id: &str) {
        fs::write(
            self.home
                .join(format!(".claude/projects/-Users-me-app/{id}.jsonl")),
            "{\"type\":\"user\"}\n",
        )
        .unwrap();
    }
    /// tk1 (account aaa) and tk2 (account bbb), both assigned, tk2 in use.
    fn two_profiles(&self) {
        self.d.setup("tk1").unwrap();
        self.sessions("tk1", "aaa/a0a");
        self.d.assign("tk1", "aaa/a0a").unwrap();
        self.d.add("tk2").unwrap();
        self.sessions("tk2", "bbb/b0b");
        self.d.assign("tk2", "bbb/b0b").unwrap();
    }
    fn backups_named(&self, prefix: &str) -> Vec<PathBuf> {
        fs::read_dir(self.root().join("_backups"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.file_name().unwrap().to_string_lossy().starts_with(prefix))
            .collect()
    }
}

fn code(result: Result<impl std::fmt::Debug>) -> &'static str {
    match result {
        Err(Error::Desktop { code, .. }) => code,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

#[test]
fn setup_makes_the_app_folder_the_first_profile_after_a_full_copy() {
    let w = world();
    assert_eq!(w.d.overview().state, "notSetUp");
    w.d.setup("tk1").unwrap();
    let app_data = &w.d.paths().app_data;
    assert!(fs::symlink_metadata(app_data)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(w.d.current().unwrap(), "tk1");
    assert_eq!(
        fs::read(w.root().join("tk1/Cookies")).unwrap(),
        b"signed in as A"
    );
    let snapshot = &w.backups_named("pre-setup-tk1-")[0];
    assert!(
        snapshot.join("profile/Cache/blob").exists(),
        "setup copies everything"
    );
    assert_eq!(read_json(&snapshot.join("manifest.json"))["complete"], true);
    // Claude was not running, so it is not opened.
    assert_eq!(w.calls(), ["quit"]);
    assert_eq!(code(w.d.setup("tk1")), "desktopAlreadySetUp");
}

#[test]
fn add_makes_an_empty_profile_backs_up_the_one_in_use_and_opens_claude() {
    let w = world();
    w.d.setup("tk1").unwrap();
    fs::create_dir_all(w.root().join("_shared/vm_bundles")).unwrap();
    w.d.add("tk2").unwrap();
    assert_eq!(w.d.current().unwrap(), "tk2");
    assert!(
        !w.root().join("tk2/Cookies").exists(),
        "a new profile carries no login"
    );
    assert!(fs::symlink_metadata(w.root().join("tk2/vm_bundles"))
        .unwrap()
        .file_type()
        .is_symlink());
    let backup = &w.backups_named("tk1-")[0];
    assert!(backup.join("profile/Cookies").exists());
    assert!(
        !backup.join("profile/Cache").exists(),
        "caches stay out of a backup"
    );
    assert_eq!(w.calls().last(), Some(&"open"));
    assert_eq!(code(w.d.add("tk2")), "desktopProfileExists");
    assert_eq!(code(w.d.add("sync")), "desktopInvalidName");
}

#[test]
fn switch_quits_backs_up_relinks_and_opens_again() {
    let w = world();
    w.d.setup("tk1").unwrap();
    w.d.add("tk2").unwrap();
    *w.d.app.running.borrow_mut() = true;
    w.d.app.calls.borrow_mut().clear();
    let report = w.d.switch("tk1").unwrap();
    assert!(report.switched);
    assert_eq!((report.from.as_str(), report.to.as_str()), ("tk2", "tk1"));
    assert!(report.backup.unwrap().join("manifest.json").exists());
    assert!(report.sync.is_none(), "auto-sync is off by default");
    assert_eq!(w.d.current().unwrap(), "tk1");
    assert_eq!(w.calls(), ["quit", "open"]);
}

#[test]
fn switching_to_the_profile_in_use_only_opens_claude() {
    let w = world();
    w.d.setup("tk1").unwrap();
    let report = w.d.switch("tk1").unwrap();
    assert!(!report.switched);
    assert_eq!(w.calls(), ["quit", "open"]);
    assert_eq!(code(w.d.switch("nobody")), "desktopUnknownProfile");
}

#[test]
fn an_app_that_will_not_quit_stops_the_switch_before_anything_changes() {
    let w = world_with(FakeApp {
        refuses_to_quit: true,
        ..FakeApp::default()
    });
    w.d.setup("tk1").unwrap();
    w.d.add("tk2").unwrap(); // opens Claude, which now will not quit
    assert_eq!(code(w.d.switch("tk1")), "desktopAppWontQuit");
    assert_eq!(w.d.current().unwrap(), "tk2");
}

#[test]
fn the_lock_keeps_one_change_at_a_time() {
    let w = world();
    w.d.setup("tk1").unwrap();
    let held = w.d.lock().unwrap();
    assert_eq!(code(w.d.switch("tk1")), "desktopBusy");
    drop(held);
    w.d.switch("tk1").unwrap();
}

#[test]
fn an_account_is_assigned_to_one_profile_only() {
    let w = world();
    w.d.setup("tk1").unwrap();
    assert_eq!(code(w.d.assign("tk1", "aaa/a0a")), "desktopNoSessions");
    w.sessions("tk1", "aaa/a0a");
    assert_eq!(code(w.d.assign("tk1", "aaa/c0c")), "desktopUnknownPair");
    w.d.assign("tk1", "aaa/a0a").unwrap();
    assert_eq!(w.d.mapping("tk1").unwrap().as_deref(), Some("aaa/a0a"));
    w.d.add("tk2").unwrap();
    // Signed in to the same account again by mistake.
    w.sessions("tk2", "aaa/a0a");
    assert_eq!(code(w.d.assign("tk2", "aaa/a0a")), "desktopAccountTaken");
}

#[test]
fn reassigning_keeps_the_old_file_and_turns_auto_sync_off() {
    let w = world();
    w.two_profiles();
    w.d.set_autosync("tk2", true).unwrap();
    w.sessions("tk2", "bbb/d0d");
    w.d.assign("tk2", "bbb/d0d").unwrap();
    assert!(!w.d.autosync("tk2").unwrap().enabled);
    assert!(!w.backups_named("tk2.account-").is_empty());
    w.d.unassign("tk2").unwrap();
    assert_eq!(w.d.mapping("tk2").unwrap(), None);
}

#[test]
fn sync_adds_cards_with_the_target_identity_and_only_once() {
    let w = world();
    w.two_profiles();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_1.json",
        json!({"cliSessionId": "s1", "accountUuid": "aaa", "organizationUuid": "a0a", "title": "Fix it"}),
    );
    w.transcript("s1");

    let preview = w.d.preview(None, None).unwrap();
    assert!(preview.dry_run);
    assert_eq!(preview.added, 1);
    assert!(!w
        .root()
        .join("tk2")
        .join(SESSIONS)
        .join("bbb/b0b/local_1.json")
        .exists());

    let report = w.d.sync(None, None).unwrap();
    assert_eq!((report.target.as_str(), report.added), ("tk2", 1));
    assert_eq!(report.sources[0].name, "tk1");
    let written = read_json(
        &w.root()
            .join("tk2")
            .join(SESSIONS)
            .join("bbb/b0b/local_1.json"),
    );
    assert_eq!(written["accountUuid"], "bbb");
    assert_eq!(written["organizationUuid"], "b0b");
    assert_eq!(written["title"], "Fix it");
    let backup = Path::new(report.backup.as_deref().unwrap());
    let manifest = read_json(&backup.join("manifest.json"));
    assert_eq!(manifest["added"], json!(["local_1.json"]));
    assert!(
        !backup.join("cards").exists(),
        "a sync's backup lists the cards it adds, it does not copy the folder"
    );
    // The source card is as it was.
    let source = read_json(
        &w.root()
            .join("tk1")
            .join(SESSIONS)
            .join("aaa/a0a/local_1.json"),
    );
    assert_eq!(source["accountUuid"], "aaa");

    let again = w.d.sync(None, None).unwrap();
    assert_eq!(again.added, 0);
    assert_eq!(
        again.skipped,
        [Skipped {
            reason: "alreadyPresent",
            count: 1
        }]
    );
}

#[test]
fn sync_leaves_out_what_it_cannot_carry_over_safely() {
    let w = world();
    w.two_profiles();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_a.json",
        json!({"cliSessionId": "no-transcript", "accountUuid": "aaa"}),
    );
    w.card(
        "tk1",
        "aaa/a0a",
        "local_b.json",
        json!({"title": "no session id"}),
    );
    w.card(
        "tk1",
        "aaa/a0a",
        "local_c.json",
        json!({"cliSessionId": "s3", "note": "see aaa"}),
    );
    fs::write(
        w.sessions("tk1", "aaa/a0a").join("local_d.json"),
        b"{\"a\":1,\"a\":2}",
    )
    .unwrap();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_e.json",
        json!({"cliSessionId": "s5"}),
    );
    w.card(
        "tk2",
        "bbb/b0b",
        "local_e.json",
        json!({"cliSessionId": "other"}),
    );
    for id in ["s3", "s5"] {
        w.transcript(id);
    }
    let report = w.d.sync(None, None).unwrap();
    assert_eq!(report.added, 0);
    let skipped: BTreeMap<_, _> = report.skipped.iter().map(|s| (s.reason, s.count)).collect();
    assert_eq!(
        skipped,
        BTreeMap::from([
            ("filenameConflict", 1),
            ("invalidCard", 2),
            ("missingTranscript", 1),
            ("noSessionId", 1),
        ])
    );
    assert_eq!(report.details.len(), 2);
}

#[test]
fn auto_sync_runs_when_switching_in() {
    let w = world();
    w.two_profiles();
    let on = w.d.set_autosync("tk2", true).unwrap();
    assert_eq!(on.sources, ["tk1"]);
    w.d.switch("tk1").unwrap();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_9.json",
        json!({"cliSessionId": "s9", "accountUuid": "aaa"}),
    );
    w.transcript("s9");
    let report = w.d.switch("tk2").unwrap();
    let synced = report.sync.expect("auto-sync is on").unwrap();
    assert_eq!(synced.added, 1);
    assert!(w
        .root()
        .join("tk2")
        .join(SESSIONS)
        .join("bbb/b0b/local_9.json")
        .exists());
    w.d.set_autosync("tk2", false).unwrap();
    assert_eq!(w.d.autosync("tk2").unwrap(), AutoSync::default());
}

#[test]
fn auto_sync_needs_an_account_on_both_sides() {
    let w = world();
    w.d.setup("tk1").unwrap();
    w.d.add("tk2").unwrap();
    assert_eq!(code(w.d.set_autosync("tk2", true)), "desktopNotAssigned");
    w.sessions("tk2", "bbb/b0b");
    w.d.assign("tk2", "bbb/b0b").unwrap();
    assert_eq!(code(w.d.set_autosync("tk2", true)), "desktopNoSources");
}

#[test]
fn the_overview_says_what_each_profile_has() {
    let w = world();
    assert_eq!(w.d.overview().state, "notSetUp");
    w.two_profiles();
    w.d.set_autosync("tk2", true).unwrap();
    w.card(
        "tk1",
        "aaa/a0a",
        "local_1.json",
        json!({"cliSessionId": "s1"}),
    );
    let o = w.d.overview();
    assert_eq!(o.state, "ready");
    assert_eq!(o.current.as_deref(), Some("tk2"));
    let names: Vec<_> = o.profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["tk1", "tk2"]);
    assert_eq!(o.profiles[0].account.as_deref(), Some("aaa/a0a"));
    assert_eq!(
        o.profiles[0].pairs,
        [PairView {
            pair: "aaa/a0a".into(),
            cards: 1
        }]
    );
    assert!(o.profiles[1].current);
    assert_eq!(o.profiles[1].autosync.sources, ["tk1"]);
}

#[test]
fn a_display_name_is_kept_beside_the_account_and_cleared_to_the_backups() {
    let w = world();
    w.d.setup("tk1").unwrap();
    assert_eq!(w.d.label("tk1"), None);
    w.d.set_label("tk1", Some("  Công ty  ")).unwrap();
    assert_eq!(w.d.label("tk1").as_deref(), Some("Công ty"));
    assert_eq!(w.d.overview().profiles[0].label.as_deref(), Some("Công ty"));
    assert_eq!(
        code(w.d.set_label("tk1", Some("two\nlines"))),
        "desktopInvalidLabel"
    );
    w.d.set_label("tk1", None).unwrap();
    assert_eq!(w.d.label("tk1"), None);
    assert!(!w.backups_named("tk1.label-reset-").is_empty());
}

#[test]
fn the_signed_in_account_and_its_recorded_limits_show_before_any_assignment() {
    let w = world();
    w.d.setup("tk1").unwrap();
    w.sessions("tk1", "aaa/a0a");
    w.sessions("tk1", "bbb/b0b");
    let profile = w.root().join("tk1");
    fs::write(
        profile.join("config.json"),
        r#"{"oauth:tokenCache":"opaque","lastKnownAccountUuid":"bbb","locale":"vi"}"#,
    )
    .unwrap();
    fs::write(
        profile.join("plan-usage-history.json"),
        r#"{"version":2,"samples":[
            {"t":1800000000000,"org":"a0a","u":{"fh":90,"sd":90}},
            {"t":1800000900000,"org":"b0b","u":{"fh":0,"sd":10}},
            {"t":1800001800000,"org":"b0b","u":{"fh":25,"sd":12}}]}"#,
    )
    .unwrap();
    let o = w.d.overview();
    let p = &o.profiles[0];
    assert_eq!(p.account, None);
    assert_eq!(p.signed_in.as_deref(), Some("bbb"));
    let limits = p.limits.as_ref().expect("the signed-in account's limits");
    assert_eq!(limits.read_at, 1_800_001_800);
    let percents: Vec<_> = limits
        .windows
        .iter()
        .map(|w| (w.label.as_str(), w.percent))
        .collect();
    assert_eq!(percents, [("5h", 25.0), ("week", 12.0)]);

    // Once assigned, the assigned account's limits show, whoever signed in.
    w.d.assign("tk1", "aaa/a0a").unwrap();
    let o = w.d.overview();
    assert_eq!(
        o.profiles[0].limits.as_ref().unwrap().windows[0].percent,
        90.0
    );
}

#[test]
fn no_claude_desktop_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let d = Desktop::new(Paths::new(dir.path(), true), FakeApp::default()).unwrap();
    assert_eq!(d.overview().state, "missing");
}

#[test]
fn the_real_app_folder_is_never_replaced_by_a_link() {
    let w = world();
    fs::create_dir_all(w.root().join("tk9")).unwrap();
    assert_eq!(code(w.d.point_to("tk9")), "desktopUnsafe");
    assert!(w.d.paths().app_data.join("Cookies").exists());
}

/// The backups `<prefix><stamp>` of a profile, as switching and syncing make them.
fn stamped(w: &World, prefix: &str) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = w
        .backups_named(prefix)
        .into_iter()
        .filter(|p| is_stamp(&p.file_name().unwrap().to_string_lossy()[prefix.len()..]))
        .collect();
    found.sort();
    found
}

#[test]
fn a_backup_leaves_out_caches_at_any_depth() {
    let w = world();
    w.d.setup("tk1").unwrap();
    let partition = w.root().join("tk1/Partitions/preview");
    for dir in ["Cache", "Code Cache", "GPUCache", "Local Storage"] {
        fs::create_dir_all(partition.join(dir)).unwrap();
        fs::write(partition.join(dir).join("f"), "x").unwrap();
    }
    w.d.add("tk2").unwrap();
    let copy = stamped(&w, "tk1-")[0].join("profile/Partitions/preview");
    assert!(copy.join("Local Storage/f").exists());
    for dir in ["Cache", "Code Cache", "GPUCache"] {
        assert!(!copy.join(dir).exists(), "{dir} stays out");
    }
    assert!(
        w.root().join("tk1/Partitions/preview/Cache/f").exists(),
        "the profile keeps its own"
    );
}

#[test]
fn switching_keeps_only_the_newest_backups_of_the_profile() {
    let w = world_keeping(FakeApp::default(), 2);
    w.two_profiles();
    let history = w.root().join("_backups/tk1-20200101-000000-0000000a");
    fs::create_dir_all(&history).unwrap();
    fs::write(
        history.join("manifest.json"),
        card::to_bytes(&json!({"complete": true, "kind": "profile", "profile": "tk1", "history_included": true})),
    )
    .unwrap();
    let unknown = w.root().join("_backups/tk1-20200101-000000-0000000b");
    fs::create_dir_all(&unknown).unwrap();
    fs::create_dir_all(w.root().join("tk1-x")).unwrap();
    w.d.backup("tk1-x").unwrap();

    let mut newest = None;
    for _ in 0..4 {
        w.d.switch("tk1").unwrap();
        newest = w.d.switch("tk2").unwrap().backup;
    }
    let kept = stamped(&w, "tk1-");
    // Two of the profile's own, plus the backup with transcripts and the
    // folder without a manifest, which are never removed.
    assert_eq!(kept.len(), 4, "{kept:?}");
    assert!(newest.unwrap().exists());
    assert!(history.exists() && unknown.exists());
    assert_eq!(stamped(&w, "tk2-").len(), 2);
    assert_eq!(
        w.backups_named("tk1-x-").len(),
        1,
        "another profile's backups are its own"
    );
}

#[test]
fn keeping_none_keeps_every_backup() {
    let w = world_keeping(FakeApp::default(), 0);
    w.two_profiles();
    for _ in 0..4 {
        w.d.switch("tk1").unwrap();
        w.d.switch("tk2").unwrap();
    }
    // tk2 was made empty, so only the four switches away from it backed it up.
    assert_eq!(stamped(&w, "tk2-").len(), 4);
}

#[test]
fn clean_frees_caches_old_versions_and_old_backups_but_not_sessions() {
    let w = world_keeping(FakeApp::default(), 1);
    w.two_profiles();
    for profile in ["tk1", "tk2"] {
        let p = w.root().join(profile);
        for dir in [
            "Cache",
            "Partitions/preview/Code Cache",
            "claude-code/2.1.9",
            "claude-code/2.1.10",
        ] {
            fs::create_dir_all(p.join(dir)).unwrap();
            fs::write(p.join(dir).join("f"), "x".repeat(10_000)).unwrap();
        }
        fs::create_dir_all(p.join("Partitions/preview/Local Storage")).unwrap();
    }
    w.card(
        "tk1",
        "aaa/a0a",
        "local_1.json",
        json!({"cliSessionId": "s1"}),
    );
    w.d.switch("tk1").unwrap();
    w.d.switch("tk2").unwrap();
    // One made before keeping was this strict, as `claude-switch` 2.0.0 left them.
    let old = w.root().join("_backups/tk1-20200101-000000-0000000c");
    fs::create_dir_all(old.join("profile")).unwrap();
    fs::write(
        old.join("manifest.json"),
        card::to_bytes(&json!({"complete": true, "kind": "profile", "profile": "tk1", "history_included": false})),
    )
    .unwrap();

    let before = w.d.storage().unwrap();
    let tk1 = before.profiles.iter().find(|p| p.name == "tk1").unwrap();
    assert!(tk1.caches > 0 && tk1.old_versions > 0 && !tk1.current);
    assert!(before.old_backups > 0);

    let report = w.d.clean().unwrap();
    assert_eq!(report.profiles, ["tk1"], "the profile in use is left alone");
    assert!(report.freed > 0 && report.backups_removed > 0);
    let tk1 = w.root().join("tk1");
    assert!(!tk1.join("Cache").exists());
    assert!(!tk1.join("Partitions/preview/Code Cache").exists());
    assert!(tk1.join("Partitions/preview/Local Storage").exists());
    assert!(!tk1.join("claude-code/2.1.9").exists());
    assert!(
        tk1.join("claude-code/2.1.10").exists(),
        "the newest version stays"
    );
    assert!(tk1.join(SESSIONS).join("aaa/a0a/local_1.json").exists());
    assert!(w.root().join("tk2/Cache").exists());
    assert!(w.root().join("tk2/claude-code/2.1.9").exists());
    assert_eq!(stamped(&w, "tk1-").len(), 1);

    let after = w.d.storage().unwrap();
    assert_eq!(after.old_backups, 0);
}

#[test]
fn stamps_and_versions_are_read_strictly() {
    assert!(is_stamp("20261020-091500-1a2b3c4d"));
    assert!(!is_stamp("x-20261020-091500-1a2b3c4d"));
    assert!(!is_stamp("20261020-091500"));
    assert_eq!(version("2.1.309"), Some(vec![2, 1, 309]));
    assert!(version("2.1.309").unwrap() > version("2.1.98").unwrap());
    assert_eq!(version("latest"), None);
}
