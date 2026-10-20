//! Claude Desktop's limits, from what it records itself: each profile's
//! `plan-usage-history.json` holds a sample every quarter of an hour or so
//! while the app runs, `{t: epoch ms, org, u: {fh, sd, xu}}`, `fh` the share
//! of the five-hour limit used and `sd` of the weekly one. No login is read
//! and nothing is asked of Anthropic, so a profile not in use shows what it
//! had when it was last open.
//!
//! The samples carry no reset times; they are estimated. A limit's period
//! starts with the first use after the last one ended, so it began between
//! the last sample before the current run of rising readings and the first
//! one in it, and ends a period later. The estimate is as good as the gap
//! between those two samples, and is marked as an estimate.

use std::path::Path;

use serde::{Deserialize, Serialize};

const HISTORY: &str = "plan-usage-history.json";
const MAX_HISTORY: u64 = 8 * 1024 * 1024;
const FIVE_HOURS: i64 = 5 * 3600;
const WEEK: i64 = 7 * 24 * 3600;
/// A fall this small is the readings wavering (rounding), not a reset:
/// a reset takes a limit back to near nothing.
const WAVER: f64 = 2.0;

/// One limit as the window shows it; the same shape as a Codex limit.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LimitWindow {
    /// `5h` or `week`.
    pub label: String,
    pub percent: f64,
    /// Estimated; none when the history does not tell.
    pub resets_at: Option<i64>,
    /// When the start hides in a gap of the history, only the latest it can
    /// reset by is known: a period started by the first reading seen.
    pub resets_by: Option<i64>,
    pub length_secs: Option<i64>,
    pub estimated: bool,
}

/// A profile's limits as last recorded.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    /// When Claude Desktop recorded them, epoch seconds.
    pub read_at: i64,
    pub windows: Vec<LimitWindow>,
}

#[derive(Deserialize)]
struct History {
    #[serde(default)]
    samples: Vec<Sample>,
}

#[derive(Deserialize)]
struct Sample {
    t: i64,
    org: String,
    #[serde(default)]
    u: Used,
}

#[derive(Deserialize, Default)]
struct Used {
    fh: Option<f64>,
    sd: Option<f64>,
}

/// One reading: epoch seconds and the share used.
type Reading = (i64, f64);

/// The limits of `org` recorded in `profile`, or none when there are none
/// or the history cannot be read.
pub fn read(profile: &Path, org: &str) -> Option<Limits> {
    let path = profile.join(HISTORY);
    let meta = std::fs::symlink_metadata(&path).ok()?;
    if !meta.is_file() || meta.len() > MAX_HISTORY {
        return None;
    }
    let history: History = serde_json::from_slice(&std::fs::read(&path).ok()?).ok()?;
    let mut samples: Vec<&Sample> = history.samples.iter().filter(|s| s.org == org).collect();
    samples.sort_by_key(|s| s.t);
    let latest = samples.last()?;
    // Milliseconds, as written; seconds would be a date in 1970.
    let secs = |t: i64| if t > 100_000_000_000 { t / 1000 } else { t };
    let series = |pick: fn(&Used) -> Option<f64>| -> Vec<Reading> {
        samples
            .iter()
            .filter_map(|s| pick(&s.u).map(|v| (secs(s.t), v)))
            .collect()
    };
    let mut windows = Vec::new();
    // A limit the latest sample leaves out is one this plan no longer has.
    if latest.u.fh.is_some() {
        windows.extend(window("5h", &series(|u| u.fh), FIVE_HOURS, 30 * 60));
    }
    if latest.u.sd.is_some() {
        windows.extend(window("week", &series(|u| u.sd), WEEK, 24 * 3600));
    }
    Some(Limits {
        read_at: secs(latest.t),
        windows,
    })
}

/// The latest reading of one limit, with its reset estimated from the run
/// of readings it belongs to. `max_gap` is how far apart the samples around
/// the start may be for the estimate to stand.
fn window(label: &str, readings: &[Reading], length: i64, max_gap: i64) -> Option<LimitWindow> {
    let &(at, percent) = readings.last()?;
    let (resets_at, resets_by) = match estimate_reset(readings, length, max_gap) {
        Some(Reset::At(reset)) => (Some(reset).filter(|&r| r > at), None),
        Some(Reset::By(reset)) => (None, Some(reset).filter(|&r| r > at)),
        None => (None, None),
    };
    Some(LimitWindow {
        label: label.into(),
        percent,
        resets_at,
        resets_by,
        length_secs: Some(length),
        estimated: true,
    })
}

/// When the period the latest reading falls in ends: its start lies between
/// the first reading of the current run (used, never falling by more than a
/// waver, within one period of the latest) and the one before it.
#[derive(Debug, PartialEq)]
enum Reset {
    /// About then.
    At(i64),
    /// At the latest then: the period started somewhere in a gap.
    By(i64),
}

fn estimate_reset(readings: &[Reading], length: i64, max_gap: i64) -> Option<Reset> {
    let last = readings.len().checked_sub(1)?;
    let (latest_at, latest) = readings[last];
    if latest <= 0.0 {
        return None; // nothing used: no period runs
    }
    let mut first = last;
    while first > 0 {
        let (before_at, before) = readings[first - 1];
        let in_run =
            before > 0.0 && before <= readings[first].1 + WAVER && latest_at - before_at < length;
        if !in_run {
            break;
        }
        first -= 1;
    }
    // The run must have a reading before it, to bound its start.
    let (before_at, _) = *readings.get(first.checked_sub(1)?)?;
    let (first_at, _) = readings[first];
    if first_at - before_at > max_gap {
        return Some(Reset::By(first_at + length));
    }
    Some(Reset::At((before_at + first_at) / 2 + length))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: i64 = 60;
    const T0: i64 = 1_800_000_000;

    fn at(minutes: &[(i64, f64)]) -> Vec<Reading> {
        minutes.iter().map(|&(m, v)| (T0 + m * MIN, v)).collect()
    }

    #[test]
    fn a_period_ends_five_hours_after_the_first_use_seen() {
        // Idle at 0, used from somewhere between minute 10 and 20.
        let readings = at(&[(0, 0.0), (10, 0.0), (20, 4.0), (35, 9.0), (50, 30.0)]);
        assert_eq!(
            estimate_reset(&readings, FIVE_HOURS, 30 * MIN),
            Some(Reset::At(T0 + 15 * MIN + FIVE_HOURS))
        );
    }

    #[test]
    fn a_fall_in_the_readings_is_a_reset_and_starts_a_new_run() {
        let readings = at(&[(0, 60.0), (15, 100.0), (30, 100.0), (45, 3.0), (60, 12.0)]);
        // The new period began between minutes 30 and 45.
        assert_eq!(
            estimate_reset(&readings, FIVE_HOURS, 30 * MIN),
            Some(Reset::At(T0 + 37 * MIN + 30 + FIVE_HOURS))
        );
    }

    #[test]
    fn a_reading_that_wavers_down_a_point_stays_in_its_run() {
        let readings = at(&[(0, 0.0), (15, 21.0), (100, 20.0), (115, 24.0)]);
        assert_eq!(
            estimate_reset(&readings, FIVE_HOURS, 30 * MIN),
            Some(Reset::At(T0 + 7 * MIN + 30 + FIVE_HOURS))
        );
    }

    #[test]
    fn nothing_used_has_no_reset() {
        let readings = at(&[(0, 20.0), (15, 0.0)]);
        assert_eq!(estimate_reset(&readings, FIVE_HOURS, 30 * MIN), None);
    }

    #[test]
    fn a_start_hidden_in_a_long_gap_gives_only_the_latest_reset() {
        // The app was closed for three hours before the first use was seen.
        let readings = at(&[(0, 0.0), (180, 25.0), (195, 30.0)]);
        assert_eq!(
            estimate_reset(&readings, FIVE_HOURS, 30 * MIN),
            Some(Reset::By(T0 + 180 * MIN + FIVE_HOURS))
        );
        // Nor is one before the history begins.
        assert_eq!(
            estimate_reset(&at(&[(0, 25.0), (15, 30.0)]), FIVE_HOURS, 30 * MIN),
            None
        );
    }

    #[test]
    fn a_run_never_reaches_back_further_than_a_period() {
        // Used without a pause for six hours: the start is not six hours back.
        let readings: Vec<Reading> = (0..=24).map(|i| (T0 + i * 15 * MIN, 50.0)).collect();
        let Some(Reset::At(reset)) = estimate_reset(&readings, FIVE_HOURS, 30 * MIN) else {
            panic!("no estimate");
        };
        assert!(reset > T0 + 6 * 3600 - 15 * MIN, "{}", reset - T0);
    }

    fn history(dir: &Path, samples: &str) {
        std::fs::write(
            dir.join(HISTORY),
            format!(r#"{{"version":2,"samples":[{samples}]}}"#),
        )
        .unwrap();
    }

    #[test]
    fn the_latest_readings_of_the_profiles_organization_are_its_limits() {
        let dir = tempfile::tempdir().unwrap();
        let ms = |m: i64| (T0 + m * MIN) * 1000;
        history(
            dir.path(),
            &format!(
                r#"{{"t":{},"org":"o1","u":{{"fh":0,"sd":40}}}},
                {{"t":{},"org":"o1","u":{{"fh":20,"sd":42,"xu":0}}}},
                {{"t":{},"org":"o2","u":{{"fh":90,"sd":90}}}},
                {{"t":{},"org":"o1","u":{{"fh":35,"sd":44}}}}"#,
                ms(0),
                ms(15),
                ms(20),
                ms(30)
            ),
        );
        let limits = read(dir.path(), "o1").unwrap();
        assert_eq!(limits.read_at, T0 + 30 * MIN);
        let five = &limits.windows[0];
        assert_eq!((five.label.as_str(), five.percent), ("5h", 35.0));
        assert_eq!(five.resets_at, Some(T0 + 7 * MIN + 30 + FIVE_HOURS));
        assert!(five.estimated);
        let week = &limits.windows[1];
        assert_eq!((week.label.as_str(), week.percent), ("week", 44.0));
        // The weekly run reaches back past the history's start: no estimate.
        assert_eq!(week.resets_at, None);
        assert_eq!(read(dir.path(), "nobody"), None);
    }

    #[test]
    fn a_limit_the_latest_sample_leaves_out_is_not_shown() {
        let dir = tempfile::tempdir().unwrap();
        history(
            dir.path(),
            r#"{"t":1800000000000,"org":"o1","u":{"fh":12}}"#,
        );
        let limits = read(dir.path(), "o1").unwrap();
        let labels: Vec<_> = limits.windows.iter().map(|w| w.label.as_str()).collect();
        assert_eq!(labels, ["5h"]);
    }

    #[test]
    fn no_history_or_an_unreadable_one_is_no_limits() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read(dir.path(), "o1"), None);
        std::fs::write(dir.path().join(HISTORY), "not json").unwrap();
        assert_eq!(read(dir.path(), "o1"), None);
    }
}
