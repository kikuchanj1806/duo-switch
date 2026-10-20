//! What the app does by itself, decided from what it already knows: when
//! usage is read next, and which run-outs have not been notified yet. Asks
//! nobody and writes nothing; `app.rs` acts on the answers.

use std::collections::HashSet;

use pitstop_core::engine::AccountUsage;
use pitstop_core::usage::UsageReport;

use crate::model::{AccountView, Suggestion, ToolView, UsageView, WindowView};

/// How long a reading that worked stands in for reads that failed.
pub const KEEP_GOOD_FOR: i64 = 15 * 60;

/// When a read asked for at `now` may start: now, or once `READ_SOONEST`
/// has passed since the last one started at `tried_at`.
pub fn read_start(tried_at: Option<i64>, now: i64) -> i64 {
    tried_at.map_or(now, |t| (t + READ_SOONEST).max(now))
}

/// Where usage reads stand. Every read asks for every account, and asking
/// more often than every `READ_SOONEST` gets reads turned away, so a read
/// asked for sooner waits for its turn and one asked for while another runs
/// follows it. None is dropped, and asks that pile up are served by one read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reads {
    pub reading: bool,
    /// When the read waiting for its turn starts.
    pub queued: Option<i64>,
    /// When the last read started, worked or not.
    pub tried_at: Option<i64>,
    /// Asked for while a read ran, and whether without waiting.
    pub again: Option<bool>,
}

/// What to do about a read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadPlan {
    Nothing,
    Start,
    /// Wait until then, then ask [`Reads::turn_came`].
    WaitUntil(i64),
}

impl Reads {
    /// Whether a read runs or waits for its turn.
    pub fn busy(&self) -> bool {
        self.reading || self.queued.is_some()
    }

    /// A read asked for at `now`; `now_anyway` (an account just added, with
    /// no reading at all) skips the wait.
    pub fn ask(&mut self, now_anyway: bool, now: i64) -> ReadPlan {
        if self.reading {
            self.again = Some(self.again.unwrap_or(false) || now_anyway);
            return ReadPlan::Nothing;
        }
        if self.queued.is_some() {
            if !now_anyway {
                return ReadPlan::Nothing;
            }
            // Served now: the waiting read finds nothing left to do.
            self.queued = None;
        }
        let start = if now_anyway {
            now
        } else {
            read_start(self.tried_at, now)
        };
        if start > now {
            self.queued = Some(start);
            ReadPlan::WaitUntil(start)
        } else {
            self.reading = true;
            self.tried_at = Some(now);
            ReadPlan::Start
        }
    }

    /// The turn of the read waiting until `due` came at `now`.
    pub fn turn_came(&mut self, due: i64, now: i64) -> ReadPlan {
        if self.queued != Some(due) {
            return ReadPlan::Nothing; // served meanwhile
        }
        self.queued = None;
        if self.reading {
            self.again = Some(self.again.unwrap_or(false));
            return ReadPlan::Nothing;
        }
        self.reading = true;
        self.tried_at = Some(now);
        ReadPlan::Start
    }

    /// The running read ended at `now`, or never got going: what was asked
    /// for meanwhile follows.
    pub fn ended(&mut self, now: i64) -> ReadPlan {
        self.reading = false;
        match self.again.take() {
            None => ReadPlan::Nothing,
            Some(now_anyway) => self.ask(now_anyway, now),
        }
    }
}

/// What a read that came back at `now` leaves: the readings, failed ones
/// standing in for by the last that worked while that is recent, and when
/// usage was last read. A reading kept from before stays dated from before,
/// so `updated_at` only moves when nothing was kept; every reading kept is
/// then from a read at or after it.
pub fn merge_read(
    old: &[AccountUsage],
    updated_at: Option<i64>,
    new: Vec<AccountUsage>,
    now: i64,
) -> (Vec<AccountUsage>, Option<i64>) {
    let recent = updated_at.is_some_and(|at| now - at <= KEEP_GOOD_FOR);
    let (usage, kept) = if recent {
        keep_last_good(old, new)
    } else {
        (new, false)
    };
    (usage, if kept { updated_at } else { Some(now) })
}

/// A new usage read, where an account's reading failed (rate limited,
/// offline) keeping the last one that worked: a passing refusal of the usage
/// service is no reason to forget the numbers. Also says whether any was
/// kept, so the read is not taken as a fresh one.
pub fn keep_last_good(old: &[AccountUsage], new: Vec<AccountUsage>) -> (Vec<AccountUsage>, bool) {
    let mut kept = false;
    let usage = new
        .into_iter()
        .map(|read| {
            if !matches!(read.report, UsageReport::Failed(_)) {
                return read;
            }
            match old.iter().find(|o| {
                o.provider == read.provider
                    && o.name == read.name
                    && matches!(o.report, UsageReport::Ok(_))
            }) {
                Some(good) => {
                    kept = true;
                    good.clone()
                }
                None => read,
            }
        })
        .collect();
    (usage, kept)
}

/// How often usage is read when no reset is near.
pub const READ_EVERY: i64 = 5 * 60;
/// The soonest one read follows another.
pub const READ_SOONEST: i64 = 30;
/// How long after a reset its fresh reading is asked for.
const AFTER_RESET: i64 = 30;

/// Seconds until the next read when every read failed for want of a network:
/// sooner than the usual five minutes, so usage shows soon after it is back.
pub const READ_OFFLINE: i64 = 60;

/// Seconds until the next usage read: shortly after the nearest reset still
/// to come, so a limit that starts afresh shows it within a minute, and
/// otherwise every five minutes.
pub fn next_read_after(resets: impl Iterator<Item = i64>, now: i64) -> i64 {
    resets
        .filter(|&at| at > now)
        .map(|at| at + AFTER_RESET - now)
        .min()
        .map_or(READ_EVERY, |wait| wait.clamp(READ_SOONEST, READ_EVERY))
}

/// The share of a limit used as it stands at `now`: one past its reset has
/// started afresh, whatever the last reading said.
pub fn used_now(w: &WindowView, now: i64) -> f64 {
    if w.resets_at.is_some_and(|at| at <= now) {
        0.0
    } else {
        w.percent
    }
}

fn windows_of(account: &AccountView) -> Option<&[WindowView]> {
    match &account.usage {
        Some(UsageView::Ok { windows }) => Some(windows),
        _ => None,
    }
}

/// A run-out to notify: the account in use of `provider` has used up
/// `window`, and `to` still has room.
#[derive(Debug, Clone, PartialEq)]
pub struct Notice {
    pub key: String,
    pub provider: &'static str,
    pub from: String,
    pub to: String,
    pub window: String,
}

/// The suggestions not notified yet. `sent` holds the keys already notified.
pub fn run_out_notices(
    tools: &[ToolView],
    suggestions: &[Suggestion],
    sent: &HashSet<String>,
    now: i64,
) -> Vec<Notice> {
    suggestions
        .iter()
        .filter_map(|s| {
            let tool = tools.iter().find(|t| t.id == s.provider)?;
            let active = tool.accounts.iter().find(|a| a.name == s.from)?;
            let window = windows_of(active)?.iter().find(|w| w.label == s.window)?;
            // A reading from before the limit reset says nothing about now.
            if window.resets_at.is_some_and(|at| at <= now) {
                return None;
            }
            let resets_at = window.resets_at;
            let key = format!(
                "{}|{}|{}|{}",
                s.provider,
                s.from,
                s.window,
                resets_at.unwrap_or(0)
            );
            (!sent.contains(&key)).then(|| Notice {
                key,
                provider: s.provider,
                from: s.from.clone(),
                to: s.to.clone(),
                window: s.window.clone(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LiveView;

    const NOW: i64 = 1_800_000_000;

    #[test]
    fn asks_for_reads_are_never_dropped_and_pile_up_into_one() {
        let mut r = Reads::default();
        assert_eq!(r.ask(false, NOW), ReadPlan::Start);
        // asked while it runs: one read follows, after its turn
        assert_eq!(r.ask(false, NOW + 5), ReadPlan::Nothing);
        assert_eq!(r.ask(false, NOW + 6), ReadPlan::Nothing);
        assert!(r.busy());
        let due = NOW + READ_SOONEST;
        assert_eq!(r.ended(NOW + 10), ReadPlan::WaitUntil(due));
        // more asks while it waits are served by it
        assert_eq!(r.ask(false, NOW + 12), ReadPlan::Nothing);
        assert!(r.busy());
        assert_eq!(r.turn_came(due, due), ReadPlan::Start);
        assert_eq!(r.ended(due + 3), ReadPlan::Nothing);
        assert!(!r.busy());
        // long after, a read starts at once
        assert_eq!(r.ask(false, due + 600), ReadPlan::Start);
    }

    #[test]
    fn an_account_just_added_is_read_without_waiting_its_turn() {
        let mut r = Reads::default();
        assert_eq!(r.ask(false, NOW), ReadPlan::Start);
        assert_eq!(r.ended(NOW + 2), ReadPlan::Nothing);
        let due = NOW + READ_SOONEST;
        assert_eq!(r.ask(false, NOW + 3), ReadPlan::WaitUntil(due));
        // a read now serves the waiting one, which then does nothing
        assert_eq!(r.ask(true, NOW + 4), ReadPlan::Start);
        assert_eq!(r.turn_came(due, due), ReadPlan::Nothing);
        // asked while a read runs, it follows that read without waiting
        assert_eq!(r.ask(true, NOW + 5), ReadPlan::Nothing);
        assert_eq!(r.ended(NOW + 6), ReadPlan::Start);
    }

    #[test]
    fn a_turn_that_comes_while_a_read_runs_follows_it() {
        let mut r = Reads::default();
        assert_eq!(r.ask(false, NOW), ReadPlan::Start);
        assert_eq!(r.ended(NOW + 1), ReadPlan::Nothing);
        let due = NOW + READ_SOONEST;
        assert_eq!(r.ask(false, NOW + 2), ReadPlan::WaitUntil(due));
        // a read got going just before the turn came
        r.reading = true;
        r.tried_at = Some(due - 1);
        assert_eq!(r.turn_came(due, due), ReadPlan::Nothing);
        assert_eq!(r.again, Some(false));
        assert_eq!(
            r.ended(due + 1),
            ReadPlan::WaitUntil(due - 1 + READ_SOONEST)
        );
    }

    #[test]
    fn a_read_too_soon_after_the_last_waits_for_its_turn() {
        assert_eq!(read_start(None, NOW), NOW);
        assert_eq!(read_start(Some(NOW - 10), NOW), NOW - 10 + READ_SOONEST);
        assert_eq!(read_start(Some(NOW - READ_SOONEST), NOW), NOW);
        assert_eq!(read_start(Some(NOW - 600), NOW), NOW);
    }

    #[test]
    fn a_kept_reading_stays_dated_from_before_and_only_while_recent() {
        use pitstop_core::usage::{Usage, UsageReport};
        use pitstop_core::ProviderId;
        let read = |report: UsageReport| AccountUsage {
            provider: ProviderId::Codex,
            name: "ben2".into(),
            report,
        };
        let good = UsageReport::Ok(Usage { windows: vec![] });
        let failed = || UsageReport::Failed(pitstop_core::usage::RATE_LIMITED.into());
        let old = vec![read(good.clone())];

        let at = NOW - 14 * 60;
        let (usage, updated) = merge_read(&old, Some(at), vec![read(failed())], NOW);
        assert_eq!(usage[0].report, good);
        assert_eq!(updated, Some(at), "kept, so not dated now");

        let at = NOW - 15 * 60 - 1;
        let (usage, updated) = merge_read(&old, Some(at), vec![read(failed())], NOW);
        assert_eq!(usage[0].report, failed());
        assert_eq!(updated, Some(NOW));

        let (usage, updated) = merge_read(&old, Some(at), vec![read(good.clone())], NOW);
        assert_eq!(usage[0].report, good);
        assert_eq!(updated, Some(NOW));
        assert_eq!(
            merge_read(&[], None, vec![read(failed())], NOW).1,
            Some(NOW)
        );
    }

    #[test]
    fn a_failed_read_keeps_the_last_reading_that_worked() {
        use pitstop_core::usage::{Usage, UsageReport};
        use pitstop_core::ProviderId;
        let read = |name: &str, report: UsageReport| AccountUsage {
            provider: ProviderId::Codex,
            name: name.into(),
            report,
        };
        let good = UsageReport::Ok(Usage { windows: vec![] });
        let failed = || UsageReport::Failed(pitstop_core::usage::RATE_LIMITED.into());
        let old = vec![
            read("ben2", good.clone()),
            read("ben3", UsageReport::Expired),
        ];
        let new = vec![
            read("ben2", failed()),
            read("ben3", failed()),
            read("ben1", failed()),
        ];
        let (kept, any) = keep_last_good(&old, new);
        assert!(any);
        assert_eq!(kept[0].report, good);
        // nothing that worked to keep: the failure shows
        assert_eq!(kept[1].report, failed());
        assert_eq!(kept[2].report, failed());
        // a refusal or a lapse is news, not a passing failure
        let (kept, any) = keep_last_good(&old, vec![read("ben2", UsageReport::Unauthorized)]);
        assert!(!any);
        assert_eq!(kept[0].report, UsageReport::Unauthorized);
    }

    fn window(label: &str, percent: f64, resets_in: Option<i64>) -> WindowView {
        WindowView {
            label: label.into(),
            percent,
            resets_at: resets_in.map(|s| NOW + s),
            length_secs: None,
        }
    }

    fn account(name: &str, in_use: bool, usage: Option<UsageView>) -> AccountView {
        AccountView {
            name: name.into(),
            label: None,
            plan: None,
            in_use,
            parked_at: None,
            usage,
        }
    }

    fn ok(windows: Vec<WindowView>) -> Option<UsageView> {
        Some(UsageView::Ok { windows })
    }

    fn codex(accounts: Vec<AccountView>) -> ToolView {
        ToolView {
            id: "codex",
            name: "Codex",
            installed: Some(true),
            app_running: None,
            live: LiveView::Account {
                name: "ben1".into(),
            },
            accounts,
        }
    }

    // ---- automatic switch

    // ---- run-out notices

    #[test]
    fn a_run_out_is_notified_once_per_limit_and_reset() {
        let tools = vec![codex(vec![
            account("ben1", true, ok(vec![window("5h", 100.0, Some(600))])),
            account("ben2", false, ok(vec![window("5h", 21.0, None)])),
        ])];
        let suggestions = vec![Suggestion {
            provider: "codex",
            from: "ben1".into(),
            to: "ben2".into(),
            window: "5h".into(),
        }];
        let first = run_out_notices(&tools, &suggestions, &HashSet::new(), NOW);
        assert_eq!(first.len(), 1);
        assert_eq!(
            (
                first[0].from.as_str(),
                first[0].to.as_str(),
                first[0].window.as_str()
            ),
            ("ben1", "ben2", "5h")
        );
        let sent: HashSet<String> = first.iter().map(|n| n.key.clone()).collect();
        assert!(run_out_notices(&tools, &suggestions, &sent, NOW).is_empty());
    }

    // ---- readings older than a reset

    #[test]
    fn a_limit_past_its_reset_is_not_notified_until_read_again() {
        let tool = codex(vec![
            account("ben1", true, ok(vec![window("5h", 100.0, Some(-60))])),
            account("ben2", false, ok(vec![window("5h", 10.0, None)])),
        ]);
        let suggestions = vec![Suggestion {
            provider: "codex",
            from: "ben1".into(),
            to: "ben2".into(),
            window: "5h".into(),
        }];
        assert!(run_out_notices(&[tool], &suggestions, &HashSet::new(), NOW).is_empty());
    }

    #[test]
    fn the_next_read_follows_the_next_reset_closely_but_never_too_often() {
        assert_eq!(next_read_after([NOW + 100].into_iter(), NOW), 130);
        assert_eq!(
            next_read_after([NOW + 3600, NOW + 200].into_iter(), NOW),
            230
        );
        assert_eq!(next_read_after([NOW - 10].into_iter(), NOW), READ_EVERY);
        assert_eq!(next_read_after(std::iter::empty(), NOW), READ_EVERY);
        assert_eq!(next_read_after([NOW + 2].into_iter(), NOW), 32);
        assert_eq!(
            next_read_after([NOW - 5, NOW + 1].into_iter(), NOW),
            READ_SOONEST.max(31)
        );
        assert_eq!(next_read_after([NOW + 1000].into_iter(), NOW), READ_EVERY);
    }
}
