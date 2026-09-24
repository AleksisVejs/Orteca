//! How much of each plan's rolling allowance is already used, in the CLI's own
//! words. Asked before a run. `claude -p /usage` answered locally for free until
//! 2.1.273, which sends it to the model instead: one small haiku call whose
//! `rate_limit_event` carries the reading. `codex app-server` answers
//! `account/rateLimits/read` without starting a thread. Both verified against
//! claude 2.1.269 and codex-cli 0.154.0 on 2026-09-13.
//!
//! Neither answer is a credential read: Orteca asks the CLI, as it does for auth.

use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use serde_json::{json, Value};

use super::{which, ProviderId};
use crate::proc::{self, Line};

/// One rolling window, e.g. Claude's "session" or Codex's "5-hour".
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub label: String,
    pub used_percent: f64,
    /// Unix seconds. Codex reports one.
    pub resets_at: Option<i64>,
    /// Claude reports words in the user's own time zone, kept verbatim.
    pub resets_text: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Limits {
    pub id: ProviderId,
    pub windows: Vec<Window>,
    /// Why there are no windows. An unread limit is never shown as an empty one.
    pub unavailable: Option<String>,
}

/// Starting a CLI is two seconds before it says anything; `/usage` also reads
/// the local session history. A reading that takes longer is not worth waiting on.
const DEADLINE: Duration = Duration::from_secs(30);

/// `/usage` needs slash commands, so this is the one claude call without
/// `--disable-slash-commands`. User settings stay out, as they do for runs.
const CLAUDE_ARGS: &[&str] = &[
    "-p",
    "/usage",
    "--output-format",
    "stream-json",
    "--verbose",
    "--setting-sources",
    "project,local",
    "--strict-mcp-config",
    // The same two the classify call runs with: no tool schemas billed on a
    // call that uses none, and no session file left behind for each reading.
    "--tools",
    "",
    "--no-session-persistence",
    "--max-turns",
    "1",
    // Newer CLIs send `/usage` to the model; keep that call on the cheapest one.
    "--model",
    "haiku",
];

/// The last reading a Claude run's own stream carried.
///
/// Every model call sends a `rate_limit_event` before its answer, so a run that
/// has just finished has already been told what `/usage` would cost another
/// haiku call and two seconds of CLI start-up to ask. The UI polls limits on a
/// timer, so that call was being paid for over and over beside runs that
/// answered it for free. A reading Orteca had to ask for is kept here too:
/// the timer asking once a minute was sixty haiku calls an hour against the
/// very allowance it reports.
static STREAMED: Mutex<Option<Vec<Window>>> = Mutex::new(None);

/// Keep a reading a run's stream carried. Called with every raw
/// `rate_limit_event`, whatever its status.
pub fn remember(id: ProviderId, v: &Value) {
    if id != ProviderId::Claude {
        return;
    }
    if let Some(windows) = parse_claude_event(v) {
        keep(windows);
    }
}

fn keep(windows: Vec<Window>) {
    if let Ok(mut latest) = STREAMED.lock() {
        *latest = Some(windows);
    }
}

/// The last kept reading, however old: nothing asks Claude on a timer any
/// more, so dropping it only turned a known figure into none. A window whose
/// reset has passed is back to zero.
fn streamed(id: ProviderId) -> Option<Vec<Window>> {
    if id != ProviderId::Claude {
        return None;
    }
    let latest = STREAMED.lock().ok()?;
    let windows = latest.as_ref()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    Some(
        windows
            .iter()
            .cloned()
            .map(|mut w| {
                if w.resets_at.is_some_and(|at| at <= now) {
                    w.used_percent = 0.0;
                }
                w
            })
            .collect(),
    )
}

/// `fresh` skips the kept reading and pays for a new one: the user's Refresh,
/// or the UI's two-minute timer while no run is going.
///
/// Claude has no free way to ask: since 2.1.273 `/usage` is a model call.
/// Otherwise the reading is what
/// Orteca's own Claude calls - runs, the classify call, commit drafts - were
/// told on the way, and none at all until one has run.
pub async fn read(id: ProviderId, fresh: bool) -> Limits {
    let none = |why: String| Limits {
        id,
        windows: Vec::new(),
        unavailable: Some(why),
    };
    // A run just told us. Asking again would start a process to be told the same.
    if let Some(windows) = streamed(id).filter(|_| !fresh) {
        return Limits {
            id,
            windows,
            unavailable: None,
        };
    }
    if id == ProviderId::Claude && !fresh {
        return none("not read since Orteca started. Claude reports it with every call, so the next run shows it; Refresh asks now with one small haiku call".into());
    }
    let Some(path) = which(id.program()) else {
        return none("not installed".into());
    };
    let program = path.to_string_lossy().into_owned();
    let answer = tokio::time::timeout(DEADLINE, async {
        match id {
            ProviderId::Claude => ask_claude(&program).await,
            ProviderId::Codex => ask_codex(&program).await,
        }
    })
    .await;
    match answer {
        Ok(Ok(windows)) => {
            if id == ProviderId::Claude {
                keep(windows.clone());
            }
            Limits {
                id,
                windows,
                unavailable: None,
            }
        }
        Ok(Err(why)) => none(why),
        Err(_) => none(format!(
            "{} did not answer within {}s",
            id.program(),
            DEADLINE.as_secs()
        )),
    }
}

async fn ask_claude(program: &str) -> Result<Vec<Window>, String> {
    // Never in the user's project: nobody has consented to its settings here.
    let mut run =
        proc::spawn(program, CLAUDE_ARGS, &std::env::temp_dir()).map_err(|e| e.to_string())?;
    run.close_stdin();
    let mut text = String::new();
    let mut event = None;
    while let Some(line) = run.lines.recv().await {
        match line {
            Line::Json(v) if v["type"] == "rate_limit_event" => {
                event = parse_claude_event(&v).or(event);
            }
            Line::Json(v) if v["type"] == "assistant" => {
                if let Some(t) = v["message"]["content"][0]["text"].as_str() {
                    text.push_str(t);
                }
            }
            Line::Exit(_) => break,
            _ => {}
        }
    }
    // Since 2.1.273 `/usage` goes to the model, whose text is not a reading.
    event.map_or_else(|| parse_claude(&text), Ok)
}

/// Reads a `rate_limit_event`'s `unifiedWindows`, which every model call carries.
pub fn parse_claude_event(v: &Value) -> Option<Vec<Window>> {
    let windows: Vec<Window> = v["rate_limit_info"]["unifiedWindows"]
        .as_object()?
        .iter()
        .filter_map(|(key, w)| {
            Some(Window {
                label: match key.as_str() {
                    "five_hour" => "session".into(),
                    "seven_day" => "week".into(),
                    k => k.replace('_', " "),
                },
                used_percent: (w["utilization"].as_f64()? * 100.0).round(),
                resets_at: w["resetsAt"].as_i64(),
                resets_text: None,
            })
        })
        .collect();
    (!windows.is_empty()).then_some(windows)
}

async fn ask_codex(program: &str) -> Result<Vec<Window>, String> {
    // The app server never exits on its own; dropping `run` closes its job.
    let mut run =
        proc::spawn(program, &["app-server"], &std::env::temp_dir()).map_err(|e| e.to_string())?;
    for message in [
        json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "orteca", "version": env!("CARGO_PKG_VERSION")}}}),
        json!({"method": "initialized"}),
        json!({"id": 2, "method": "account/rateLimits/read"}),
    ] {
        run.send_line(&message.to_string())
            .await
            .map_err(|e| e.to_string())?;
    }
    while let Some(line) = run.lines.recv().await {
        match line {
            Line::Json(v) if v["id"] == 2 => return parse_codex(&v),
            Line::Exit(code) => {
                return Err(format!(
                    "codex app-server exited ({code:?}) before answering"
                ))
            }
            _ => {}
        }
    }
    Err("codex app-server closed without answering".into())
}

/// Reads lines like `Current session: 66% used · resets Sep 13, 3:50pm (Europe/Kyiv)`.
/// The rest of `/usage` is local session statistics and is ignored.
pub fn parse_claude(text: &str) -> Result<Vec<Window>, String> {
    let windows: Vec<Window> = text
        .lines()
        .filter_map(|line| {
            let (label, figures) = line.trim().strip_prefix("Current ")?.split_once(':')?;
            let (used, after) = figures.split_once("% used")?;
            Some(Window {
                label: label.trim().to_string(),
                used_percent: used.trim().parse().ok()?,
                resets_at: None,
                resets_text: after
                    .split_once("resets")
                    .map(|(_, when)| when.trim().to_string())
                    .filter(|when| !when.is_empty()),
            })
        })
        .collect();
    if !windows.is_empty() {
        Ok(windows)
    } else if text.to_ascii_lowercase().contains("api key") {
        Err("billed per token by API key, so there is no plan limit to read".into())
    } else {
        Err("claude /usage printed nothing Orteca could read".into())
    }
}

/// Reads the `account/rateLimits/read` response: `primary` and `secondary`.
pub fn parse_codex(response: &Value) -> Result<Vec<Window>, String> {
    if let Some(message) = response["error"]["message"].as_str() {
        return Err(format!("codex: {message}"));
    }
    let limits = &response["result"]["rateLimits"];
    let windows: Vec<Window> = ["primary", "secondary"]
        .iter()
        .filter_map(|key| {
            let w = &limits[*key];
            Some(Window {
                label: window_label(w["windowDurationMins"].as_u64()?),
                used_percent: w["usedPercent"].as_f64()?,
                resets_at: w["resetsAt"].as_i64(),
                resets_text: None,
            })
        })
        .collect();
    if windows.is_empty() {
        Err("codex reported no rate-limit windows".into())
    } else {
        Ok(windows)
    }
}

fn window_label(minutes: u64) -> String {
    match minutes {
        10080 => "week".into(),
        m if m % 60 == 0 => format!("{}-hour", m / 60),
        m => format!("{m}-minute"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A kept reading never expires, but a window that has since reset reads zero.
    #[test]
    fn a_kept_reading_stays_and_a_reset_window_reads_zero() {
        let window = |label: &str, resets_at| Window {
            label: label.into(),
            used_percent: 80.0,
            resets_at: Some(resets_at),
            resets_text: None,
        };
        keep(vec![window("session", 1), window("week", i64::MAX)]);
        let kept = streamed(ProviderId::Claude).unwrap();
        assert_eq!(kept[0].used_percent, 0.0);
        assert_eq!(kept[1].used_percent, 80.0);
    }

    /// Recorded from claude 2.1.269 on a subscription, 2026-09-13.
    const CLAUDE_USAGE: &str =
        "You are currently using your subscription to power your Claude Code usage\n\n\
        Current session: 66% used · resets Sep 13, 3:50pm (Europe/Kyiv)\n\
        Current week (all models): 69% used · resets Sep 17, 11am (Europe/Kyiv)\n\n\
        What's contributing to your limits usage?\n\
        Last 24h · 965 requests · 51 sessions\n  60% of your usage was at >150k context\n";

    /// A Claude reading costs a model call, so only the user's Refresh pays
    /// for one: an ordinary read is a kept reading or a reason, never a CLI start.
    #[tokio::test]
    async fn claude_is_not_asked_unless_the_user_refreshes() {
        let started = Instant::now();
        let read = read(ProviderId::Claude, false).await;
        let kept = !read.windows.is_empty();
        assert!(kept || read.unavailable.as_deref().is_some_and(|w| w.contains("Refresh")), "{read:?}");
        assert!(started.elapsed() < Duration::from_secs(1), "a CLI was started");
    }

    #[test]
    fn claude_usage_text_becomes_windows() {
        let windows = parse_claude(CLAUDE_USAGE).unwrap();
        assert_eq!(windows.len(), 2, "the session statistics are not windows");
        assert_eq!(windows[0].label, "session");
        assert_eq!(windows[0].used_percent, 66.0);
        assert_eq!(
            windows[0].resets_text.as_deref(),
            Some("Sep 13, 3:50pm (Europe/Kyiv)")
        );
        assert_eq!(windows[1].label, "week (all models)");
        assert_eq!(windows[1].used_percent, 69.0);
    }

    #[test]
    fn claude_without_windows_says_why_and_never_reports_zero() {
        let key =
            parse_claude("You are currently using your API key to power your Claude Code usage")
                .unwrap_err();
        assert!(key.contains("API key"), "{key}");
        assert!(
            parse_claude("").is_err(),
            "no text is no reading, not 0% used"
        );
    }

    /// Recorded from claude 2.1.273, 2026-09-16.
    #[test]
    fn claude_rate_limit_event_becomes_windows() {
        let v = json!({"type": "rate_limit_event", "rate_limit_info": {"status": "allowed_warning",
            "unifiedWindows": {"five_hour": {"utilization": 0.24, "resetsAt": 1789580400},
                               "seven_day": {"utilization": 0.83, "resetsAt": 1789632000}}}});
        let windows = parse_claude_event(&v).unwrap();
        assert_eq!(
            windows[0],
            Window {
                label: "session".into(),
                used_percent: 24.0,
                resets_at: Some(1789580400),
                resets_text: None
            }
        );
        assert_eq!(windows[1].label, "week");
        assert_eq!(windows[1].used_percent, 83.0);
        assert!(parse_claude_event(&json!({"type": "rate_limit_event"})).is_none());
    }

    /// Recorded from codex-cli 0.154.0 on a Plus plan, 2026-09-13.
    #[test]
    fn codex_rate_limits_become_windows() {
        let response = json!({"id": 2, "result": {"ordinaryUsageAllowed": true, "rateLimits": {
            "limitId": "codex",
            "primary": {"usedPercent": 26, "windowDurationMins": 300, "resetsAt": 1789304323},
            "secondary": {"usedPercent": 39, "windowDurationMins": 10080, "resetsAt": 1789805328},
            "credits": {"hasCredits": false, "unlimited": false, "balance": "0"}, "planType": "plus"}}});
        let windows = parse_codex(&response).unwrap();
        assert_eq!(
            windows[0],
            Window {
                label: "5-hour".into(),
                used_percent: 26.0,
                resets_at: Some(1789304323),
                resets_text: None
            }
        );
        assert_eq!(windows[1].label, "week");
        assert_eq!(windows[1].used_percent, 39.0);
    }

    #[test]
    fn a_codex_error_is_unavailable_with_its_message() {
        let why =
            parse_codex(&json!({"id": 2, "error": {"code": -32600, "message": "not logged in"}}))
                .unwrap_err();
        assert!(why.contains("not logged in"), "{why}");
        assert!(parse_codex(&json!({"id": 2, "result": {}})).is_err());
    }
}
