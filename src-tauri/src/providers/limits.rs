//! How much of each plan's rolling allowance is already used, in the CLI's own
//! words. Asked before a run and never paid for: `claude -p /usage` answers
//! locally with a zero-token synthetic message, and `codex app-server` answers
//! `account/rateLimits/read` without starting a thread. Both verified against
//! claude 2.1.269 and codex-cli 0.154.0 on 2026-09-13.
//!
//! Neither answer is a credential read: Orteca asks the CLI, as it does for auth.

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
    "--max-turns",
    "1",
];

pub async fn read(id: ProviderId) -> Limits {
    let none = |why: String| Limits { id, windows: Vec::new(), unavailable: Some(why) };
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
        Ok(Ok(windows)) => Limits { id, windows, unavailable: None },
        Ok(Err(why)) => none(why),
        Err(_) => none(format!("{} did not answer within {}s", id.program(), DEADLINE.as_secs())),
    }
}

async fn ask_claude(program: &str) -> Result<Vec<Window>, String> {
    // Never in the user's project: nobody has consented to its settings here.
    let mut run = proc::spawn(program, CLAUDE_ARGS, &std::env::temp_dir()).map_err(|e| e.to_string())?;
    run.close_stdin();
    let mut text = String::new();
    while let Some(line) = run.lines.recv().await {
        match line {
            Line::Json(v) if v["type"] == "assistant" => {
                if let Some(t) = v["message"]["content"][0]["text"].as_str() {
                    text.push_str(t);
                }
            }
            Line::Exit(_) => break,
            _ => {}
        }
    }
    parse_claude(&text)
}

async fn ask_codex(program: &str) -> Result<Vec<Window>, String> {
    // The app server never exits on its own; dropping `run` closes its job.
    let mut run = proc::spawn(program, &["app-server"], &std::env::temp_dir()).map_err(|e| e.to_string())?;
    for message in [
        json!({"id": 1, "method": "initialize", "params": {"clientInfo": {"name": "orteca", "version": env!("CARGO_PKG_VERSION")}}}),
        json!({"method": "initialized"}),
        json!({"id": 2, "method": "account/rateLimits/read"}),
    ] {
        run.send_line(&message.to_string()).await.map_err(|e| e.to_string())?;
    }
    while let Some(line) = run.lines.recv().await {
        match line {
            Line::Json(v) if v["id"] == 2 => return parse_codex(&v),
            Line::Exit(code) => return Err(format!("codex app-server exited ({code:?}) before answering")),
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

    /// Recorded from claude 2.1.269 on a subscription, 2026-09-13.
    const CLAUDE_USAGE: &str = "You are currently using your subscription to power your Claude Code usage\n\n\
        Current session: 66% used · resets Sep 13, 3:50pm (Europe/Kyiv)\n\
        Current week (all models): 69% used · resets Sep 17, 11am (Europe/Kyiv)\n\n\
        What's contributing to your limits usage?\n\
        Last 24h · 965 requests · 51 sessions\n  60% of your usage was at >150k context\n";

    #[test]
    fn claude_usage_text_becomes_windows() {
        let windows = parse_claude(CLAUDE_USAGE).unwrap();
        assert_eq!(windows.len(), 2, "the session statistics are not windows");
        assert_eq!(windows[0].label, "session");
        assert_eq!(windows[0].used_percent, 66.0);
        assert_eq!(windows[0].resets_text.as_deref(), Some("Sep 13, 3:50pm (Europe/Kyiv)"));
        assert_eq!(windows[1].label, "week (all models)");
        assert_eq!(windows[1].used_percent, 69.0);
    }

    #[test]
    fn claude_without_windows_says_why_and_never_reports_zero() {
        let key = parse_claude("You are currently using your API key to power your Claude Code usage").unwrap_err();
        assert!(key.contains("API key"), "{key}");
        assert!(parse_claude("").is_err(), "no text is no reading, not 0% used");
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
        assert_eq!(windows[0], Window { label: "5-hour".into(), used_percent: 26.0, resets_at: Some(1789304323), resets_text: None });
        assert_eq!(windows[1].label, "week");
        assert_eq!(windows[1].used_percent, 39.0);
    }

    #[test]
    fn a_codex_error_is_unavailable_with_its_message() {
        let why = parse_codex(&json!({"id": 2, "error": {"code": -32600, "message": "not logged in"}})).unwrap_err();
        assert!(why.contains("not logged in"), "{why}");
        assert!(parse_codex(&json!({"id": 2, "result": {}})).is_err());
    }
}
