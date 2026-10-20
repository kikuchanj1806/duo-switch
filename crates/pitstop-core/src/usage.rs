//! Best-effort usage, read the way Codex itself reads it:
//! `GET https://chatgpt.com/backend-api/wham/usage` with the account's
//! `ChatGPT-Account-ID`, as the Codex CLI sends it.
//!
//! Pitstop never refreshes a token to read usage. Refreshing rotates the
//! refresh token, and the live login is the tool's to renew — two holders
//! renewing one chain would break it for both. An account whose access token
//! has lapsed shows as expired until it is switched to and the tool renews it.

use std::time::Duration;

use serde_json::Value;

pub const CODEX_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";

/// One rate-limit window, e.g. the five-hour session or the weekly limit.
#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub label: String,
    pub percent: f64,
    /// Epoch seconds.
    pub resets_at: Option<i64>,
    /// How long the window runs, when known, so its pace can be judged.
    pub length_secs: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Usage {
    pub windows: Vec<Window>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum UsageReport {
    Ok(Usage),
    /// The parked access token has lapsed; switching to the account lets
    /// Codex renew it.
    Expired,
    /// The service refused the token (revoked or signed out elsewhere).
    Unauthorized,
    Failed(String),
}

/// What a read the service turned away for coming too often came to.
pub const RATE_LIMITED: &str = "rate limited, try again later";

#[derive(Debug)]
pub enum FetchError {
    Unauthorized,
    Other(String),
}

/// The network calls, as a seam so the engine is testable offline.
pub trait UsageApi: Sync {
    /// OpenAI's answer for a Codex access token and its ChatGPT account.
    fn codex(&self, access_token: &str, account_id: &str) -> Result<Value, FetchError>;
}

/// OpenAI over the network.
pub struct Http {
    agent: ureq::Agent,
}

impl Http {
    pub fn new() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(10)))
            .http_status_as_error(false)
            .build()
            .into();
        Self { agent }
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

/// Usage comes back as JSON on 200, a refusal on 401/403.
fn answer(
    response: Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    service: &str,
) -> Result<Value, FetchError> {
    let mut response = response.map_err(|e| FetchError::Other(format!("network: {e}")))?;
    match response.status().as_u16() {
        200 => {
            let body = response
                .body_mut()
                .read_to_string()
                .map_err(|e| FetchError::Other(format!("network: {e}")))?;
            serde_json::from_str(&body)
                .map_err(|e| FetchError::Other(format!("unreadable answer: {e}")))
        }
        401 | 403 => Err(FetchError::Unauthorized),
        429 => Err(FetchError::Other(RATE_LIMITED.into())),
        status => Err(FetchError::Other(format!("{service} answered {status}"))),
    }
}

impl UsageApi for Http {
    /// A standalone GET: no model request, no quota
    /// spent. `ChatGPT-Account-ID` is required.
    fn codex(&self, access_token: &str, account_id: &str) -> Result<Value, FetchError> {
        let response = self
            .agent
            .get(CODEX_USAGE_URL)
            .header("Authorization", format!("Bearer {access_token}"))
            .header("ChatGPT-Account-ID", account_id)
            .call();
        answer(response, "OpenAI")
    }
}

/// The access token and ChatGPT account id inside a codex vault blob, which
/// the usage request needs together. Deliberately not `Debug`.
pub(crate) enum CodexToken {
    Valid {
        access_token: String,
        account_id: String,
    },
    Expired,
    Missing,
}

/// The access token of a codex blob unless its `exp` claim is already past,
/// with the account id the request has to name (`tokens.account_id`, else
/// the id_token's `chatgpt_account_id`).
pub(crate) fn codex_token(blob: &[u8], now_secs: i64) -> CodexToken {
    use crate::provider::codex::jwt_claims;

    let Ok(auth) = serde_json::from_slice::<Value>(blob) else {
        return CodexToken::Missing;
    };
    let tokens = &auth["tokens"];
    let Some(access_token) = tokens.get("access_token").and_then(Value::as_str) else {
        return CodexToken::Missing;
    };
    let account_id = tokens
        .get("account_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
        .or_else(|| {
            let claims = jwt_claims(tokens.get("id_token")?.as_str()?)?;
            claims
                .pointer("/https:~1~1api.openai.com~1auth/chatgpt_account_id")?
                .as_str()
                .map(str::to_owned)
        });
    let Some(account_id) = account_id else {
        return CodexToken::Missing;
    };
    match jwt_claims(access_token).and_then(|c| c.get("exp")?.as_i64()) {
        Some(exp) if exp <= now_secs => CodexToken::Expired,
        _ => CodexToken::Valid {
            access_token: access_token.to_owned(),
            account_id,
        },
    }
}

/// Read OpenAI's answer: the windows under `rate_limit`, either of which
/// can be null. A window whose numbers do not make sense is dropped.
pub fn parse_codex(body: &Value) -> Usage {
    let limits = &body["rate_limit"];
    let windows = ["primary_window", "secondary_window"]
        .into_iter()
        .filter_map(|which| codex_window(limits.get(which)?, which))
        .collect();
    Usage { windows }
}

fn codex_window(value: &Value, which: &str) -> Option<Window> {
    let percent = value.get("used_percent").and_then(Value::as_f64)?;
    if !percent.is_finite() || percent < 0.0 {
        return None;
    }
    let length = value
        .get("limit_window_seconds")
        .and_then(Value::as_i64)
        .filter(|s| *s > 0);
    Some(Window {
        label: window_name(length, which),
        percent,
        resets_at: value.get("reset_at").and_then(moment),
        length_secs: length,
    })
}

/// `5h` and `week` for the usual windows; anything else is named by its own
/// length.
fn window_name(seconds: Option<i64>, which: &str) -> String {
    const HOUR: i64 = 3_600;
    match seconds {
        Some(FIVE_HOURS) => "5h".into(),
        Some(WEEK) => "week".into(),
        Some(s) if s % (24 * HOUR) == 0 => format!("{}d", s / (24 * HOUR)),
        Some(s) if s % HOUR == 0 => format!("{}h", s / HOUR),
        Some(s) if s % 60 == 0 => format!("{}m", s / 60),
        Some(s) => format!("{s}s"),
        None => which.trim_end_matches("_window").to_owned(),
    }
}

/// Epoch seconds, sent as a number or as an RFC 3339 timestamp.
fn moment(value: &Value) -> Option<i64> {
    if let Some(secs) = value.as_i64() {
        // Milliseconds would be a date thousands of years out.
        return (secs > 0 && secs < 100_000_000_000).then_some(secs);
    }
    value.as_str().and_then(parse_rfc3339)
}

const FIVE_HOURS: i64 = 18_000;
const WEEK: i64 = 604_800;

pub(crate) fn codex_report(result: Result<Value, FetchError>) -> UsageReport {
    report(result, parse_codex)
}

fn report(result: Result<Value, FetchError>, parse: fn(&Value) -> Usage) -> UsageReport {
    match result {
        Ok(body) => UsageReport::Ok(parse(&body)),
        Err(FetchError::Unauthorized) => UsageReport::Unauthorized,
        Err(FetchError::Other(msg)) => UsageReport::Failed(msg),
    }
}

impl Usage {
    /// `5h 62% (2h10m) · week 48% (4d3h)`
    pub fn summary(&self, now_secs: i64) -> String {
        if self.windows.is_empty() {
            return "no limits reported".into();
        }
        self.windows
            .iter()
            .map(|w| {
                let pct = format!("{} {:.0}%", w.label, w.percent);
                match w.resets_at {
                    Some(at) if at > now_secs => {
                        format!("{pct} ({})", short_duration(at - now_secs))
                    }
                    _ => pct,
                }
            })
            .collect::<Vec<_>>()
            .join(" · ")
    }
}

impl UsageReport {
    pub fn summary(&self, now_secs: i64) -> String {
        match self {
            UsageReport::Ok(usage) => usage.summary(now_secs),
            UsageReport::Expired => "token expired — switch to it to refresh".into(),
            UsageReport::Unauthorized => "login no longer accepted — sign in again".into(),
            UsageReport::Failed(msg) => format!("usage unavailable: {msg}"),
        }
    }
}

fn short_duration(secs: i64) -> String {
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3_600, secs % 3_600 / 60);
    match (d, h) {
        (0, 0) => format!("{}m", m.max(1)),
        (0, _) => format!("{h}h{m:02}m"),
        _ => format!("{d}d{h}h"),
    }
}

/// `2026-09-20T22:20:00.095287+00:00` / `...Z` → epoch seconds.
pub(crate) fn parse_rfc3339(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.len() < 20 || b[4] != b'-' || b[7] != b'-' || b[10] != b'T' {
        return None;
    }
    let num = |r: std::ops::Range<usize>| s.get(r)?.parse::<i64>().ok();
    let (y, mo, d) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, mi, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    let mut rest = &s[19..];
    if let Some(frac) = rest.strip_prefix('.') {
        rest = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match rest {
        "Z" | "z" => 0,
        _ => {
            let sign = match rest.as_bytes().first()? {
                b'+' => 1,
                b'-' => -1,
                _ => return None,
            };
            let oh = rest.get(1..3)?.parse::<i64>().ok()?;
            let om = rest.get(4..6)?.parse::<i64>().ok()?;
            sign * (oh * 3_600 + om * 60)
        }
    };
    Some(days_from_civil(y, mo, d) * 86_400 + h * 3_600 + mi * 60 + sec - offset)
}

/// Howard Hinnant's days-from-civil (proleptic Gregorian).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (m + if m > 2 { -3 } else { 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// OpenAI's answer as measured (Codex 0.160, 2026-09).
    fn codex_answer() -> Value {
        json!({
            "plan_type": "pro",
            "rate_limit": {
                "allowed": true,
                "limit_reached": false,
                "primary_window": {
                    "used_percent": 45,
                    "limit_window_seconds": 604_800,
                    "reset_after_seconds": 492_401,
                    "reset_at": 1_790_628_078i64
                },
                "secondary_window": null
            },
            "rate_limit_reached_type": null
        })
    }

    #[test]
    fn codex_answer_becomes_named_windows() {
        let usage = parse_codex(&codex_answer());
        assert_eq!(
            usage.windows,
            [Window {
                label: "week".into(),
                percent: 45.0,
                resets_at: Some(1_790_628_078),
                length_secs: Some(604_800),
            }]
        );
    }

    #[test]
    fn codex_reads_both_windows_when_both_are_filled_in() {
        let mut body = codex_answer();
        body["rate_limit"]["primary_window"] = json!({
            "used_percent": 12.5, "limit_window_seconds": 18_000, "reset_at": 1_790_100_000i64
        });
        body["rate_limit"]["secondary_window"] = json!({
            "used_percent": 30, "limit_window_seconds": 604_800, "reset_at": "2026-09-27T02:00:00Z"
        });
        let usage = parse_codex(&body);
        let read: Vec<_> = usage
            .windows
            .iter()
            .map(|w| (w.label.as_str(), w.percent, w.resets_at))
            .collect();
        assert_eq!(
            read,
            [
                ("5h", 12.5, Some(1_790_100_000)),
                ("week", 30.0, parse_rfc3339("2026-09-27T02:00:00Z"))
            ]
        );
    }

    #[test]
    fn codex_windows_of_other_lengths_are_named_by_length() {
        let mut body = codex_answer();
        body["rate_limit"]["primary_window"] =
            json!({"used_percent": 1, "limit_window_seconds": 10_800});
        body["rate_limit"]["secondary_window"] =
            json!({"used_percent": 2, "limit_window_seconds": 86_400});
        let labels: Vec<_> = parse_codex(&body)
            .windows
            .into_iter()
            .map(|w| w.label)
            .collect();
        assert_eq!(labels, ["3h", "1d"]);
    }

    #[test]
    fn codex_drops_a_window_it_cannot_read() {
        let mut body = codex_answer();
        body["rate_limit"]["primary_window"] =
            json!({"used_percent": "quite a lot", "limit_window_seconds": 18_000});
        body["rate_limit"]["secondary_window"] =
            json!({"used_percent": -1.0, "limit_window_seconds": 604_800});
        assert!(parse_codex(&body).windows.is_empty());
    }

    fn codex_blob(exp: i64, account_id: Option<&str>) -> Vec<u8> {
        use base64::Engine as _;
        let b64 = |s: &str| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(s);
        let access = format!(
            "{}.{}.{}",
            b64(r#"{"alg":"RS256"}"#),
            b64(&format!(r#"{{"exp":{exp}}}"#)),
            b64("sig")
        );
        let mut tokens = json!({"access_token": access, "id_token": "x.y.z", "refresh_token": "R"});
        if let Some(id) = account_id {
            tokens["account_id"] = json!(id);
        }
        serde_json::to_vec(&json!({"auth_mode": "chatgpt", "tokens": tokens})).unwrap()
    }

    #[test]
    fn codex_token_is_used_only_while_unexpired_and_with_its_account() {
        let now = 1_800_000_000;
        assert!(matches!(
            codex_token(&codex_blob(now + 60, Some("acc-1")), now),
            CodexToken::Valid { access_token, account_id }
                if account_id == "acc-1" && access_token.split('.').count() == 3
        ));
        assert!(matches!(
            codex_token(&codex_blob(now - 1, Some("acc-1")), now),
            CodexToken::Expired
        ));
        assert!(matches!(
            codex_token(&codex_blob(now + 60, None), now),
            CodexToken::Missing
        ));
        assert!(matches!(codex_token(b"{}", now), CodexToken::Missing));
    }

    #[test]
    fn summary_shows_percent_and_time_to_reset() {
        let now = parse_rfc3339("2026-09-20T20:10:00Z").unwrap();
        let window = |label: &str, percent, resets_at| Window {
            label: label.into(),
            percent,
            resets_at: Some(resets_at),
            length_secs: None,
        };
        let usage = Usage {
            windows: vec![
                window("5h", 62.0, now + 2 * 3_600 + 600),
                window("week", 48.0, now + 6 * 86_400 + 5 * 3_600),
            ],
        };
        assert_eq!(usage.summary(now), "5h 62% (2h10m) · week 48% (6d5h)");
    }

    #[test]
    fn rfc3339_parses_offsets_and_fractions() {
        assert_eq!(parse_rfc3339("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_rfc3339("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(
            parse_rfc3339("2000-03-01T00:00:00.5-00:00"),
            Some(951_868_800)
        );
        assert_eq!(parse_rfc3339("garbage"), None);
    }
}
