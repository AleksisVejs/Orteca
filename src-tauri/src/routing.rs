//! Deterministic routing. Pure Rust, zero tokens, no model call of any kind.
//!
//! Everything here is a function of the user's prompt plus signals that were
//! already cheap to collect: the repository's tracked paths and how the same
//! prompt fared before. Nothing in this module starts a process, and nothing in
//! it asks a provider what it thinks - a classifier that spends tokens to
//! decide whether to spend tokens has already lost the argument.
//!
//! The route is fixed *before* a provider starts. A trivial task gets exactly
//! one agent call, which verifies itself unless Orteca can run the tests after
//! it. A route never promotes itself to a longer one, and it has no call, turn
//! or token ceiling: a Review or Verify that does not pass is followed by a Fix
//! and the same check again. Reviews are independent but bounded: one review
//! can buy one fix, then deterministic verification is the completion gate.

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::codemap::FileFacts;
use crate::intent::Intent;
use crate::providers::ProviderId;

/// Two modes, as decided in the architecture. `Efficient` shifts every route
/// threshold up by two, so more work lands on the shorter routes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum Mode {
    Efficient,
    #[default]
    Balanced,
}

impl Mode {
    /// The spelling the task row stores.
    pub fn name(self) -> &'static str {
        match self {
            Self::Efficient => "efficient",
            Self::Balanced => "balanced",
        }
    }

    /// How far every threshold moves. Efficient is not a second table of
    /// routes, just the same table read two points further along.
    fn shift(self) -> u8 {
        match self {
            Self::Efficient => 2,
            Self::Balanced => 0,
        }
    }
}

/// What a stage is for. Its own type because the capability -> provider map
/// lives in one place rather than spread through the router.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Capability {
    Deep,
    Implement,
    Review,
}

impl Capability {
    /// The provider this capability would prefer, per the architecture's one
    /// mapping table. **Policy data only.** Milestone 6 executes every stage
    /// with the provider the user selected: silently sending a stage to a CLI
    /// the user has not signed into would fail a run for a reason the screen
    /// never mentioned. The preference is recorded so Milestone 7 can act on it
    /// once per-stage auth state is something the router can actually see.
    pub fn preferred_provider(self) -> ProviderId {
        match self {
            Self::Deep | Self::Review => ProviderId::Claude,
            Self::Implement => ProviderId::Codex,
        }
    }
}

/// A cheapest-capable tier for a task class, and the model each CLI runs it on.
///
/// The catalogue is research, not measurement: list prices, public coding
/// benchmarks and the Codex account's own model list as of 2026-09-13, written
/// up in architecture §4.3.4. Claude takes aliases, which follow the newest
/// model the account can use, and the id that actually ran is read back from
/// `modelUsage`. Codex has no aliases, so its slugs are pinned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Tier {
    Cheapest,
    Standard,
    Deep,
}

/// What a tier asks one CLI for: a model and a reasoning effort.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ModelChoice {
    pub model: &'static str,
    pub effort: &'static str,
}

impl Tier {
    fn up(self) -> Option<Tier> {
        match self {
            Self::Cheapest => Some(Self::Standard),
            Self::Standard => Some(Self::Deep),
            Self::Deep => None,
        }
    }

    fn down(self) -> Option<Tier> {
        match self {
            Self::Cheapest => None,
            Self::Standard => Some(Self::Cheapest),
            Self::Deep => Some(Self::Standard),
        }
    }

    // ponytail: pinned Codex slugs go stale when OpenAI retires a model; read
    // `~/.codex/models_cache.json` if that starts happening between releases.
    pub fn model(self, id: ProviderId) -> ModelChoice {
        let (model, effort) = match (id, self) {
            // Sonnet 5 at low effort, not Haiku 4.5: Haiku is half the price but
            // a generation older with no effort control, and one retry costs
            // more than the difference.
            (ProviderId::Claude, Self::Cheapest) => ("sonnet", "low"),
            (ProviderId::Claude, Self::Standard) => ("sonnet", "high"),
            // Opus 5, not Fable 5.1: twice the price for a lead that only shows
            // on the hardest benchmarks.
            (ProviderId::Claude, Self::Deep) => ("opus", "high"),
            // Luna low planned the validation refactor and fixed the duration
            // regression with full hidden-check quality (§4.3.6).
            (ProviderId::Codex, Self::Cheapest) => ("gpt-5.6-luna", "low"),
            (ProviderId::Codex, Self::Standard) => ("gpt-5.6-terra", "medium"),
            // Sol, not Astra: half the price, and Astra has no SWE-bench Pro
            // score yet.
            (ProviderId::Codex, Self::Deep) => ("gpt-5.6-sol", "high"),
        };
        ModelChoice { model, effort }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Stage {
    Plan,
    Implement,
    Review,
    Verify,
    /// Follows a Review or Verify that did not pass, on the route's own tier:
    /// it fixes what was found, and the check that failed runs again after it.
    Fix,
    /// Answers a question about the project and changes nothing.
    Answer,
}

impl Stage {
    /// The `task_events.stage` column. One spelling, used by the log and the UI.
    pub fn name(self) -> &'static str {
        match self {
            Self::Plan => "plan",
            Self::Implement => "implement",
            Self::Review => "review",
            Self::Verify => "verify",
            Self::Fix => "fix",
            Self::Answer => "answer",
        }
    }

    pub fn capability(self) -> Capability {
        match self {
            Self::Plan => Capability::Deep,
            Self::Implement | Self::Verify | Self::Fix => Capability::Implement,
            Self::Review | Self::Answer => Capability::Review,
        }
    }

    /// The artifact shape this stage is contracted to return, if any.
    ///
    /// Implement has none: it is judged by the diff, and asking a CLI to wrap a
    /// code change in a JSON envelope buys nothing. Verify and Fix do, because
    /// a stage that ends cleanly while reporting failing checks is not a pass.
    pub fn schema(self) -> Option<&'static str> {
        match self {
            Self::Plan => Some(PLAN_SCHEMA),
            Self::Review => Some(REVIEW_SCHEMA),
            Self::Verify | Self::Fix => Some(VERIFY_SCHEMA),
            Self::Implement | Self::Answer => None,
        }
    }

    /// Whether this stage is allowed to change files. A plan that edits code
    /// has skipped the review the route put after it.
    pub fn writes(self) -> bool {
        matches!(self, Self::Implement | Self::Fix)
    }
}

/// The inter-stage artifact from a Plan stage, as JSON Schema. Enforced by
/// `claude --json-schema` and `codex --output-schema`, never by reading prose
/// and hoping.
pub const PLAN_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["objective","constraints","affected_areas","implementation_steps","risks","tests_required"],"properties":{"objective":{"type":"string"},"constraints":{"type":"array","items":{"type":"string"}},"affected_areas":{"type":"array","items":{"type":"string"}},"implementation_steps":{"type":"array","items":{"type":"string"}},"risks":{"type":"array","items":{"type":"string"}},"tests_required":{"type":"array","items":{"type":"string"}}}}"#;

/// The Review artifact. `verdict: "pass"` is what lets the route end without a
/// fix call - which is where "calls avoided" is actually earned.
///
/// `fix` is described as the requirement, not the edit. A reviewer that writes
/// the patch spends the route's most expensive output tokens on code, and the
/// Fix follows it literally: one review dictated "emit `switch` for an
/// already-open path and a new `open` event for the rest" into a codebase whose
/// `open()` already did both, and the redundant event shipped.
pub const REVIEW_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["findings","verdict"],"properties":{"findings":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["severity","file","line","issue","fix"],"properties":{"severity":{"type":"string","enum":["low","medium","high"]},"file":{"type":"string"},"line":{"type":"integer"},"issue":{"type":"string","description":"The defect and the failure it causes, in one or two sentences."},"fix":{"type":"string","description":"What must become true for this finding to be resolved, in one or two sentences. State the requirement and any constraint it must keep, not the code: do not write the patch, name the functions to call, or dictate an implementation. The fixing agent reads the repository and chooses how."}}}},"verdict":{"type":"string","enum":["pass","changes_requested"]}}}"#;

/// The Verify artifact: each check that ran and whether it passed.
pub const VERIFY_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["checks","verdict"],"properties":{"checks":{"type":"array","items":{"type":"object","additionalProperties":false,"required":["command","passed","output"],"properties":{"command":{"type":"string"},"passed":{"type":"boolean"},"output":{"type":"string"}}}},"verdict":{"type":"string","enum":["pass","fail"]}}}"#;

/// Check a structured artifact against the shape its stage contracted for.
///
/// A deliberately shallow check: required keys, present and of the right JSON
/// kind. It exists so a missing or half-built artifact is *recorded as invalid*
/// rather than passed on as if it were a plan. It is not a JSON Schema
/// validator - the provider's own flag is that, and a second implementation
/// here would be one more thing to keep in step.
pub fn artifact_is_valid(stage: Stage, value: &serde_json::Value) -> bool {
    let Some(object) = value.as_object() else {
        return false;
    };
    match stage {
        Stage::Plan => {
            object
                .get("objective")
                .is_some_and(serde_json::Value::is_string)
                && [
                    "constraints",
                    "affected_areas",
                    "implementation_steps",
                    "risks",
                    "tests_required",
                ]
                .iter()
                .all(|key| object.get(*key).is_some_and(serde_json::Value::is_array))
        }
        Stage::Review => {
            object
                .get("findings")
                .is_some_and(serde_json::Value::is_array)
                && object
                    .get("verdict")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|v| v == "pass" || v == "changes_requested")
        }
        Stage::Verify | Stage::Fix => {
            object
                .get("checks")
                .is_some_and(serde_json::Value::is_array)
                && object
                    .get("verdict")
                    .and_then(serde_json::Value::as_str)
                    .is_some_and(|v| v == "pass" || v == "fail")
        }
        Stage::Implement | Stage::Answer => false,
    }
}

/// `true` when a Review or Verify artifact says the work is finished. Anything
/// that is not a valid artifact saying `pass` is not a pass, and neither is a
/// Verify or Fix pass with no check behind it or with a check that failed. A
/// Review whose findings are all `low` passes whatever its verdict: those are
/// notes, and another fix-and-review round for them is spend nobody needs.
pub fn stage_passed(stage: Stage, value: &serde_json::Value) -> bool {
    if !artifact_is_valid(stage, value) {
        return false;
    }
    if stage == Stage::Review {
        return value["verdict"] == "pass"
            || value["findings"].as_array().is_some_and(|findings| {
                !findings.is_empty() && findings.iter().all(|f| f["severity"] == "low")
            });
    }
    value["verdict"] == "pass"
        && value["checks"].as_array().is_some_and(|checks| {
            !checks.is_empty() && checks.iter().all(|c| c["passed"] == true)
        })
}

/// What the classifier read out of the prompt and the repository. Serialised
/// into the routing event whole, so a later milestone can score these decisions
/// against real outcomes without a schema change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Signals {
    pub complexity: u8,
    pub risk: u8,
    pub architecture: bool,
    pub security: bool,
    pub authz: bool,
    pub schema_change: bool,
    pub bug: bool,
    pub refactor: bool,
    pub frontend: bool,
    /// The small model supplied the job/risk classification. When false, the
    /// keyword classifier was the unavailable-model fallback.
    #[serde(default)]
    pub job_classified: bool,
    /// Repository-declared commands the user explicitly asked Orteca to run.
    #[serde(default)]
    pub requested_build: bool,
    #[serde(default)]
    pub requested_test: bool,
    #[serde(default)]
    pub requested_lint: bool,
    /// Tracked files whose path matches a word in the prompt. A count, not a
    /// judgement: it says how much of the repository the prompt points at.
    pub blast_radius: usize,
    /// How many earlier runs of this same prompt in this project ended without
    /// finishing. Two is the architecture's escalation trigger.
    pub prior_failures: u32,
    /// What the provider's smallest model read the prompt as. `None` when it
    /// was not asked (the preview) or could not tell.
    #[serde(default)]
    pub intent: Option<Intent>,
}

/// The tiers a route runs on, chosen before any provider is started. There is
/// no call, turn or token ceiling: a route runs until its checks pass (§4.3.8).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecutionBudget {
    /// Cheapest capable tier for this task class, after adaptation. Every
    /// stage of the route runs on it, Fix included: caches are per model, so
    /// switching between stages would pay for the same context twice.
    pub preferred_tier: Tier,
    /// The tier a Review runs on when it is not `preferred_tier`. Guarded work
    /// is built a tier below and reviewed on `deep`: a read-only look at a
    /// finished diff is where the stronger model earns its price.
    pub review_tier: Option<Tier>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RouteKind {
    /// A question: one read-only call whose reply is the result.
    Answer,
    /// One Implement call that inspects, edits and verifies itself, or leaves
    /// the tests to Orteca when it can run them. No Plan, no Review.
    ImplementOnce,
    Standard,
    Planned,
    Guarded,
    Escalated,
}

/// The decision, whole. Stored in `tasks.route_json` and written to the event
/// log before the first provider starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Route {
    pub kind: RouteKind,
    pub mode: Mode,
    pub stages: Vec<Stage>,
    pub budget: ExecutionBudget,
    pub signals: Signals,
    /// Why this route, in the words of the rule that chose it.
    pub reason: &'static str,
    /// Why `budget.preferred_tier`, in the same way.
    pub tier_reason: &'static str,
    /// Tracked paths the prompt appears to be about. Names only: the runner
    /// pastes the few small ones into a Codex brief, and never into Claude's,
    /// whose Edit tool makes it Read a file first anyway.
    pub candidate_paths: Vec<String>,
    /// What the first few candidate paths define and use, from the code map,
    /// in the same order; empty where the map has nothing.
    #[serde(default)]
    pub candidate_notes: Vec<String>,
    /// Earlier work in this project that bears on the prompt, for the
    /// Implement brief. Empty when nothing does.
    #[serde(default)]
    pub prior_notes: Vec<String>,
    /// Why each was chosen, saved with the route so a strange run can be
    /// traced. Never in a brief.
    #[serde(default)]
    pub prior_notes_why: Vec<String>,
    /// What each stage would have preferred to run on, recorded and not acted
    /// on. See `Capability::preferred_provider`.
    pub preferred_providers: Vec<ProviderId>,
}

impl Route {
    /// One agent call and nothing after it, so the call checks its own work.
    pub fn is_single_call(&self) -> bool {
        self.stages == [Stage::Implement]
    }

    /// `true` when the checks run before the Review, so a failed check is fixed
    /// first and the Review reads the fix.
    pub fn verifies_before_review(&self) -> bool {
        let at = |stage| self.stages.iter().position(|s| *s == stage);
        matches!((at(Stage::Verify), at(Stage::Review)), (Some(v), Some(r)) if v < r)
    }

    /// Efficient plans keep the route's model and lower only its effort. The
    /// implementation and its checks still protect the result, while keeping
    /// the same model preserves the provider's prompt cache.
    pub fn plan_model(&self, id: ProviderId) -> Option<ModelChoice> {
        if self.mode != Mode::Efficient || !self.stages.contains(&Stage::Plan) {
            return None;
        }
        let mut choice = self.budget.preferred_tier.model(id);
        choice.effort = "low";
        Some(choice)
    }

    /// Schema work gets one stronger Codex session instead of a cheaper draft
    /// followed by several expensive repairs. Both modes use the same proven
    /// Sol family; lower-consumption challengers can be benchmarked later
    /// without putting Astra on the user's allowance.
    pub fn work_model(&self, id: ProviderId) -> Option<ModelChoice> {
        let schema = (id == ProviderId::Codex && self.signals.schema_change).then_some(ModelChoice {
            model: "gpt-5.6-sol",
            effort: "medium",
        });
        // NAV_EFFORT runs Implement and Fix at another effort, for the
        // benchmark's arms. It moves the effort and never the model, and the
        // Review keeps its own tier either way, so what it measures is the
        // writing effort alone.
        let Some(effort) = bench_effort() else {
            return schema;
        };
        let mut choice = schema.unwrap_or_else(|| self.budget.preferred_tier.model(id));
        choice.effort = effort;
        Some(choice)
    }

    /// What the Review runs on when that is not the route's tier.
    pub fn review_model(&self, id: ProviderId) -> Option<ModelChoice> {
        let review_tier = self.budget.review_tier?;
        let mut choice = review_tier.model(id);
        match id {
            // Medium in every mode: the seeded regression Sonnet missed was
            // caught by Opus *medium* (§4.3.6), so high is effort the evidence
            // never asked for. On one 60-line frontend diff high spent 13.9k
            // output tokens and 173s - 60% of the run's cost, 54% of its wall
            // time - to return findings medium reaches.
            ProviderId::Claude => choice.effort = "medium",
            // Terra high found the same seeded containment defect as Sol
            // low with fewer tokens and lower latency (§4.3.6).
            ProviderId::Codex if self.mode == Mode::Efficient => {
                choice = Tier::Standard.model(id);
                choice.effort = "high";
            }
            ProviderId::Codex => {}
        }
        Some(choice)
    }
}

/// The effort `NAV_EFFORT` asks Implement and Fix for. An unset or unknown
/// value leaves the route alone: a typo must never quietly reroute a run.
///
/// It *sets* the effort and does not cap it, so on a tier that already asks
/// for less it raises. Point it at a task that lands on the tier being
/// measured, or the arm measures the opposite of what it meant to.
fn bench_effort() -> Option<&'static str> {
    match std::env::var("NAV_EFFORT").ok()?.as_str() {
        "low" => Some("low"),
        "medium" => Some("medium"),
        "high" => Some("high"),
        _ => None,
    }
}

/// Words that make a prompt complex, and what each is worth.
const COMPLEXITY: &[(&str, u8)] = &[
    ("architecture", 4),
    ("architectural", 4),
    ("redesign", 4),
    ("restructure", 4),
    ("rewrite", 4),
    ("subsystem", 3),
    ("end-to-end", 3),
    ("orchestrat", 3),
    ("migrate", 3),
    ("refactor", 3),
    ("across the", 3),
    ("throughout", 3),
    ("every file", 3),
    ("whole codebase", 4),
    ("entire codebase", 4),
    ("design", 2),
    ("implement", 2),
    ("introduce", 2),
    ("integrate", 2),
    ("concurren", 2),
    ("async", 2),
    ("performance", 2),
    ("multiple", 2),
    ("add support", 2),
    ("new module", 2),
    ("protocol", 2),
    ("state machine", 3),
    // A cause nobody has found yet. The fix may be one line; finding it is not.
    ("intermittent", 3),
    ("flaky", 3),
    ("deadlock", 3),
    ("race condition", 3),
    (" hang", 3),
    ("memory leak", 3),
    ("nondetermin", 3),
    ("sometimes", 2),
    ("randomly", 2),
];

/// Words that say a prompt is small. Subtracted, never below zero.
const TRIVIAL: &[(&str, u8)] = &[
    ("typo", 3),
    ("spelling", 3),
    ("comment", 2),
    ("docstring", 2),
    ("rename", 2),
    ("whitespace", 3),
    ("formatting", 2),
    ("wording", 2),
    ("log message", 2),
    ("bump", 2),
    ("changelog", 2),
    ("one-line", 3),
    ("one line", 3),
    ("small fix", 2),
    ("readme", 2),
];

/// Words that make a prompt risky, independent of how complex it is.
const RISK: &[(&str, u8)] = &[
    ("delete", 3),
    ("drop table", 5),
    ("production", 4),
    ("irreversible", 4),
    ("secret", 4),
    ("credential", 4),
    ("password", 4),
    ("api key", 4),
    ("encrypt", 3),
    ("sandbox", 3),
    ("permission", 3),
    ("payment", 4),
    ("billing", 3),
    ("upgrade", 2),
    ("breaking change", 3),
    ("data loss", 4),
];

const SECURITY: &[&str] = &[
    "security",
    "vulnerab",
    "exploit",
    "injection",
    "xss",
    "csrf",
    "sanitis",
    "sanitiz",
    "secret",
    "credential",
    "password",
    "api key",
    "encrypt",
    "certificate",
    "sandbox escape",
    "cve",
];

const AUTHZ: &[&str] = &[
    "authoris",
    "authoriz",
    "authentic",
    "permission",
    "access control",
    "rbac",
    "session token",
    "oauth",
    "privilege",
];

const SCHEMA: &[&str] = &[
    "migration",
    "schema",
    "database",
    "sqlite",
    "table",
    "column",
    "index on",
    "foreign key",
    "drop table",
    "alter table",
];

const ARCHITECTURE: &[&str] = &[
    "architecture",
    "architectural",
    "redesign",
    "restructure",
    "rewrite",
    "subsystem",
    "whole codebase",
    "entire codebase",
    "orchestrat",
    "state machine",
    "end-to-end",
];

const BUG: &[&str] = &[
    "bug",
    "fix",
    "broken",
    "regression",
    "crash",
    "panic",
    "fails",
    "failing",
    "wrong",
];
const REFACTOR: &[&str] = &[
    "refactor", "clean up", "cleanup", "tidy", "simplify", "extract", "rename",
];
const FRONTEND: &[&str] = &[
    "ui",
    "css",
    "vue",
    "component",
    "screen",
    "button",
    "layout",
    "style",
    "frontend",
];

/// Words too common to say anything about which files a prompt is about.
const STOPWORDS: &[&str] = &[
    "the", "and", "that", "this", "with", "from", "into", "when", "then", "than", "have", "has",
    "was", "were", "for", "not", "but", "all", "any", "each", "make", "made", "should", "would",
    "could", "must", "please", "need", "want", "file", "files", "code", "test", "tests", "line",
    "lines", "change", "changes", "add", "added", "new", "use", "using", "also", "where", "what",
    "which",
];

/// One finished task, as a note for a later brief.
#[derive(Debug, Clone)]
pub struct PastNote {
    pub id: i64,
    pub note: String,
    /// Every file the task changed.
    pub files: Vec<String>,
}

/// The repository side of the decision. Collected once, cheaply, before the
/// classifier runs - so the classifier itself stays a pure function of its
/// inputs and is testable without a repository at all.
#[derive(Debug, Clone, Default)]
pub struct RepoSignals {
    /// Tracked paths, as `git ls-files` reports them.
    pub tracked_paths: Vec<String>,
    /// Paths touched by the last 50 commits. A ranking signal, never a match
    /// on its own.
    pub recent_paths: Vec<String>,
    /// What each mapped source file defines and uses. Ranking only, never
    /// the blast radius.
    pub code_map: Vec<(String, FileFacts)>,
    /// Notes on this project's earlier finished tasks, newest first. `route`
    /// keeps the few that changed a file this prompt is about.
    pub past: Vec<PastNote>,
    pub prior_failures: u32,
    /// Route kinds and tiers that stalled in this project's recent runs on the
    /// selected provider. See `Store::stalled_tiers`.
    pub stalled_tiers: Vec<(RouteKind, Tier)>,
    /// Room left in the selected provider's tightest plan window, 0-100, as its
    /// CLI reported it before the run. `None` when it could not be read, which
    /// never counts as low.
    pub headroom: Option<f64>,
    /// The repository declares a test command Orteca can run itself, so a
    /// Verify costs no agent call.
    pub checks_locally: bool,
    /// See `Signals::intent`.
    pub intent: Option<Intent>,
    /// The small model's semantic job classification. Keywords are used only
    /// when this is absent because the call failed or was not made.
    pub job: Option<crate::intent::Job>,
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|n| haystack.contains(n))
}

fn score(haystack: &str, table: &[(&str, u8)]) -> u8 {
    table
        .iter()
        .filter(|(word, _)| haystack.contains(word))
        .map(|(_, weight)| *weight)
        .fold(0u8, u8::saturating_add)
}

/// The words in a prompt worth matching a path against. A word starting with
/// `/` is a URL, not a repository path: its segments count, down to three
/// letters, so `/api/orders` reaches `routes/api.php`.
fn nouns(prompt: &str) -> Vec<String> {
    let mut words: Vec<String> = prompt
        .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '.' && c != '/')
        .flat_map(|w| {
            if w.starts_with('/') {
                w.split(|c: char| !c.is_alphanumeric()).filter(|s| s.len() >= 3).collect::<Vec<_>>()
            } else {
                vec![w].into_iter().filter(|w| w.len() >= 4).collect::<Vec<_>>()
            }
        })
        .map(|w| w.trim_end_matches('.').to_ascii_lowercase())
        .filter(|w| !w.is_empty() && !STOPWORDS.contains(&w.as_str()))
        .collect();
    words.sort();
    words.dedup();
    // One concept, one word: "orders" goes when "order" is there.
    let all = words.clone();
    words.retain(|w| !all.iter().any(|base| base != w && names_word(w, base)));
    words
}

/// A built bundle a tracked `public/` folder or a `dist/` holds. Its symbols are
/// minified and its names hashed, so it matches many ordinary words at once and
/// outranks the one source file the prompt names.
fn generated(path: &str) -> bool {
    let built = path.ends_with(".js") || path.ends_with(".css") || path.ends_with(".map");
    (path.starts_with("public/") && built) || path.starts_with("dist/") || path.contains("/dist/")
}

const TEST_WORDS: &[&str] = &["test", "tests", "spec"];

/// What a file is named for, with its extension and any test marker removed:
/// `src/slug.rs`, `tests/test_slug.py` and `slug_test.rs` all give `slug`.
fn stem(path: &str) -> Option<String> {
    let name = path
        .rsplit('/')
        .next()?
        .split('.')
        .next()?
        .to_ascii_lowercase();
    let words: Vec<&str> = name
        .split(['_', '-'])
        .filter(|w| !w.is_empty() && !TEST_WORDS.contains(w))
        .collect();
    (!words.is_empty()).then(|| words.join("_"))
}

fn is_test(path: &str) -> bool {
    path.to_ascii_lowercase()
        .split(['/', '.', '_', '-'])
        .any(|w| TEST_WORDS.contains(&w))
}

/// The lowercase words of an identifier: `QuoteController` and
/// `quote_controller` both give `quote`, `controller`.
fn ident_words(name: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut word = String::new();
    let mut prev_lower = false;
    for c in name.chars() {
        if !c.is_alphanumeric() {
            prev_lower = false;
            words.push(std::mem::take(&mut word));
            continue;
        }
        if c.is_uppercase() && prev_lower {
            words.push(std::mem::take(&mut word));
        }
        prev_lower = c.is_lowercase() || c.is_ascii_digit();
        word.extend(c.to_lowercase());
    }
    words.push(word);
    words.retain(|w| !w.is_empty());
    words
}

/// The URL paths a prompt names, keyed as `codemap::url_key` keys strings.
fn prompt_urls(prompt: &str) -> Vec<String> {
    prompt
        .split_whitespace()
        .map(|w| w.trim_start_matches(['(', '`', '"', '\'']).trim_end_matches([')', '`', '"', '\'', ',', '.', ':', ';']))
        .filter_map(crate::codemap::url_key)
        .map(|key| format!("url:{key}"))
        .collect()
}

/// How well a URL a file mentions matches one the prompt names: the same
/// path, or one the tail of the other (a client's base URL drops `/api`),
/// scores 3; two leading segments in common, a sibling endpoint, 1.
fn url_match(prompt: &str, file: &str) -> u32 {
    let a: Vec<&str> = prompt.split('/').collect();
    let b: Vec<&str> = file.split('/').collect();
    let same = |x: &[&str], y: &[&str]| x.len() == y.len() && x.iter().zip(y).all(|(p, q)| p == q || *p == "*" || *q == "*");
    let (short, long) = if a.len() <= b.len() { (&a, &b) } else { (&b, &a) };
    if same(&a, &b) || (short.len() >= 2 && same(short, &long[long.len() - short.len()..])) {
        return 3;
    }
    let common = a.iter().zip(&b).take_while(|(p, q)| p == q && **p != "*").count();
    if common >= 2 { 1 } else { 0 }
}

/// A prompt word names an identifier word, allowing a plural.
fn names_word(noun: &str, word: &str) -> bool {
    noun.strip_prefix(word)
        .is_some_and(|rest| matches!(rest, "" | "s" | "es"))
}

/// The words of what a file defines: types, then functions and methods.
struct Defined {
    types: HashSet<String>,
    functions: HashSet<String>,
}

fn defined_words(facts: &FileFacts) -> Defined {
    let mut defined = Defined { types: HashSet::new(), functions: HashSet::new() };
    for symbol in &facts.defines {
        let into = if matches!(symbol.kind.as_str(), "function" | "method") {
            &mut defined.functions
        } else {
            &mut defined.types
        };
        into.extend(ident_words(&symbol.name));
    }
    defined
}

/// Names defined by more files than this are too common to link through.
const MAX_DEFINERS: usize = 3;

/// Which mapped files point at which, in either direction: a file that uses
/// a name is linked to the files that define it, and a path spec is linked
/// to the tracked file it resolves to.
fn links(repo: &RepoSignals) -> HashMap<&str, HashSet<&str>> {
    let tracked: HashSet<&str> = repo.tracked_paths.iter().map(String::as_str).collect();
    let mut definers: HashMap<&str, Vec<&str>> = HashMap::new();
    for (path, facts) in &repo.code_map {
        for symbol in facts.defines.iter().filter(|s| s.kind != "method") {
            definers.entry(symbol.name.as_str()).or_default().push(path.as_str());
        }
    }
    let mut links: HashMap<&str, HashSet<&str>> = HashMap::new();
    for (path, facts) in &repo.code_map {
        for name in &facts.uses {
            let targets = if name.starts_with("url:") {
                Vec::new()
            } else if name.contains('/') {
                resolve(path, name, &tracked, &repo.tracked_paths)
            } else {
                definers
                    .get(name.as_str())
                    .filter(|d| d.len() <= MAX_DEFINERS)
                    .cloned()
                    .unwrap_or_default()
            };
            for target in targets.into_iter().filter(|t| *t != path.as_str()) {
                links.entry(path.as_str()).or_default().insert(target);
                links.entry(target).or_default().insert(path.as_str());
            }
        }
    }
    links
}

/// The tracked files a path spec from `from` points at.
fn resolve<'a>(from: &str, spec: &str, tracked: &HashSet<&'a str>, all: &'a [String]) -> Vec<&'a str> {
    if let Some(file) = spec.strip_prefix("lang/*/") {
        return all
            .iter()
            .map(String::as_str)
            .filter(|p| p.starts_with("lang/") && p.ends_with(file) && p.matches('/').count() == 2)
            .collect();
    }
    let bases: Vec<String> = if spec.starts_with("./") || spec.starts_with("../") {
        let mut parts: Vec<&str> = from.split('/').collect();
        parts.pop();
        for part in spec.split('/') {
            match part {
                "." => {}
                ".." => {
                    parts.pop();
                }
                _ => parts.push(part),
            }
        }
        vec![parts.join("/")]
    } else if let Some(rest) = spec.strip_prefix("@/").or_else(|| spec.strip_prefix("~/")) {
        // ponytail: the two usual aliases, not the bundler's config.
        vec![format!("resources/js/{rest}"), format!("src/{rest}")]
    } else {
        vec![spec.to_string()]
    };
    const ENDINGS: &[&str] = &["", ".ts", ".js", ".vue", ".tsx", ".jsx", "/index.ts", "/index.js"];
    bases
        .iter()
        .flat_map(|base| ENDINGS.iter().map(move |end| format!("{base}{end}")))
        .filter_map(|p| tracked.get(p.as_str()).copied())
        .take(1)
        .collect()
}

/// How many paths a brief names, and how many of them are kept for tests.
const LISTED: usize = 10;
const TEST_SLOTS: usize = 2;

/// Seeds whose linked files are pulled into the list, and what each link to
/// one is worth in units of the prompt's rarest word. Tuned on recorded bench
/// runs with `scripts/bench/navstats.mjs --recall`.
const SEEDS: usize = 4;
const HOP: f64 = 3.0;

/// Tracked paths the prompt is about, most likely first, and how many of them
/// matched a word of the prompt. Path text, git history and the code map: no
/// file is opened here.
///
/// A file named for a prompt word scores 3, a path merely containing one 1.
/// A file defining a type named for the word scores 3 too, a function or
/// method 1; per word the better of the path and the symbols counts. A URL
/// the prompt names counts as a word, matched against URLs in the code. Each
/// word is weighted by how rare it is among tracked files, so "quote" in a
/// quoting app counts less than "resend". Being touched in the last 50
/// commits adds 2, and each link to a top seed through the map `HOP`, both in
/// units of the rarest word. A path the prompt spells out goes first. A test
/// named like a hit is listed beside it even when the prompt never mentioned
/// it. The blast radius, which measures what the prompt points at, counts
/// path matches only, as it always has.
fn candidates(prompt: &str, repo: &RepoSignals) -> (usize, Vec<String>) {
    let mut nouns = nouns(prompt);
    if nouns.is_empty() {
        return (0, Vec::new());
    }
    if !repo.code_map.is_empty() {
        nouns.extend(prompt_urls(prompt));
    }
    let urls: HashMap<&str, Vec<&str>> = repo
        .code_map
        .iter()
        .map(|(path, facts)| {
            let urls = facts.uses.iter().filter_map(|u| u.strip_prefix("url:")).collect();
            (path.as_str(), urls)
        })
        .collect();
    let defined: HashMap<&str, Defined> = repo
        .code_map
        .iter()
        .map(|(path, facts)| (path.as_str(), defined_words(facts)))
        .collect();
    // Per matched file, per noun: (by path or symbols, by path alone).
    let source: Vec<&String> = repo.tracked_paths.iter().filter(|p| !generated(p)).collect();
    let matched: Vec<(&String, Vec<(u32, u32)>)> = source
        .iter()
        .copied()
        .filter_map(|path| {
            let lower = path.to_ascii_lowercase();
            let name = lower.rsplit('/').next();
            let stem = stem(path);
            // A test's method names describe behaviour in the prompt's own
            // words, so they would outrank the code under test.
            let defined = defined.get(path.as_str()).filter(|_| !is_test(path));
            let per_noun: Vec<(u32, u32)> = nouns
                .iter()
                .map(|noun| {
                    if let Some(url) = noun.strip_prefix("url:") {
                        let files = urls.get(path.as_str()).map_or(&[][..], Vec::as_slice);
                        return (files.iter().map(|f| url_match(url, f)).max().unwrap_or(0), 0);
                    }
                    let by_path = if name == Some(noun.as_str()) || stem.as_deref().is_some_and(|s| names_word(noun, s)) {
                        3
                    } else if lower.contains(noun.as_str()) {
                        1
                    } else {
                        0
                    };
                    let names = |words: &HashSet<String>| words.iter().any(|w| names_word(noun, w));
                    let by_symbol = match defined {
                        Some(d) if names(&d.types) => 3,
                        Some(d) if names(&d.functions) => 1,
                        _ => 0,
                    };
                    (by_path.max(by_symbol), by_path)
                })
                .collect();
            per_noun.iter().any(|(s, _)| *s > 0).then_some((path, per_noun))
        })
        .collect();
    let stems = |by_path_only: bool| -> HashSet<String> {
        matched
            .iter()
            .filter(|(path, m)| !is_test(path) && (!by_path_only || m.iter().any(|(_, p)| *p > 0)))
            .filter_map(|(path, _)| stem(path))
            .collect()
    };
    let paired_to = |stems: &HashSet<String>, path: &str| is_test(path) && stem(path).is_some_and(|s| stems.contains(&s));
    // A paired test never widens the blast radius, even one the prompt named.
    let path_stems = stems(true);
    let hits = matched
        .iter()
        .filter(|(path, m)| m.iter().any(|(_, p)| *p > 0) && !paired_to(&path_stems, path))
        .count();

    let total = source.len() as f64;
    let weights: Vec<f64> = (0..nouns.len())
        .map(|i| {
            let df = matched.iter().filter(|(_, m)| m[i].0 > 0).count().max(1) as f64;
            (1.0 + total / df).ln()
        })
        .collect();
    let unit = weights.iter().copied().fold(0.0, f64::max);
    let recent: HashSet<&String> = repo.recent_paths.iter().collect();
    let mut scored: Vec<(f64, &String)> = matched
        .iter()
        .map(|(path, m)| (m.iter().zip(&weights).map(|((s, _), w)| *s as f64 * w).sum(), *path))
        .collect();
    fn add<'a>(scored: &mut Vec<(f64, &'a String)>, extra: Vec<(f64, &'a String)>) {
        let listed: HashSet<&String> = scored.iter().map(|(_, p)| *p).collect();
        scored.extend(extra.into_iter().filter(|(_, p)| !listed.contains(p)));
    }

    if !repo.code_map.is_empty() {
        let bonus = |p: &String| if recent.contains(p) { 2.0 * unit } else { 0.0 };
        let mut seeds: Vec<(f64, &String)> = scored
            .iter()
            .filter(|(_, p)| !is_test(p))
            .map(|(s, p)| (s + bonus(p), *p))
            .collect();
        seeds.sort_by(|a, b| b.0.total_cmp(&a.0));
        let links = links(repo);
        let by_path: HashMap<&str, &String> = repo.tracked_paths.iter().map(|p| (p.as_str(), p)).collect();
        // A file linked to several seeds is likelier than one linked to one.
        let mut linked: HashMap<&str, f64> = HashMap::new();
        for seed in seeds.iter().take(SEEDS).filter_map(|(_, seed)| links.get(seed.as_str())) {
            for path in seed {
                *linked.entry(path).or_default() += HOP * unit;
            }
        }
        for (score, path) in &mut scored {
            *score += linked.get(path.as_str()).copied().unwrap_or_default();
        }
        let extra = linked.iter().filter_map(|(p, s)| by_path.get(p).map(|p| (*s, *p))).collect();
        add(&mut scored, extra);
    }

    let all_stems = stems(false);
    let tests = repo
        .tracked_paths
        .iter()
        .filter(|path| paired_to(&all_stems, path))
        .map(|path| (unit, path))
        .collect();
    add(&mut scored, tests);
    for (score, path) in &mut scored {
        if recent.contains(path) {
            *score += 2.0 * unit;
        }
        // A path the prompt spells out is listed first.
        if path.contains('/') && nouns.contains(&path.to_ascii_lowercase()) {
            *score += 1000.0 * unit;
        }
    }
    // Rounded, so sums of the same weights in another order still tie. Ties
    // go shallowest and shortest first: `src/run.rs` is a likelier subject
    // than `src/deep/nested/run_helpers_generated.rs`.
    let key = |s: f64| (s * 1e6).round() as i64;
    scored.sort_by(|(a, pa), (b, pb)| {
        key(*b).cmp(&key(*a)).then_with(|| {
            (pa.matches('/').count(), pa.len(), pa).cmp(&(pb.matches('/').count(), pb.len(), pb))
        })
    });
    // The brief lists `LISTED` paths. Tests and code compete for them
    // separately, so a pile of weak code matches cannot crowd out the test
    // that exercises the endpoint, nor tests the code.
    let n_tests = scored.iter().filter(|(_, p)| is_test(p)).count();
    let test_slots = TEST_SLOTS.max(LISTED.saturating_sub(scored.len() - n_tests));
    let code_slots = (LISTED - TEST_SLOTS).max(LISTED.saturating_sub(n_tests));
    let (mut head, mut rest) = (Vec::new(), Vec::new());
    let (mut tests_in, mut code_in) = (0, 0);
    for item in scored {
        let slot = if is_test(item.1) { &mut tests_in } else { &mut code_in };
        let room = if is_test(item.1) { test_slots } else { code_slots };
        if head.len() < LISTED && *slot < room {
            *slot += 1;
            head.push(item);
        } else {
            rest.push(item);
        }
    }
    let scored: Vec<(f64, &String)> = head.into_iter().chain(rest).collect();
    (hits, scored.into_iter().map(|(_, p)| p.clone()).collect())
}

/// Read a prompt and its repository into numbers. Pure: same inputs, same
/// route, every time.
pub fn classify(prompt: &str, repo: &RepoSignals) -> (Signals, Vec<String>) {
    let text = prompt.to_ascii_lowercase();
    let words = text.split_whitespace().count();

    let mut complexity = score(&text, COMPLEXITY);
    // A long prompt is describing more work than a short one.
    if words > 60 {
        complexity = complexity.saturating_add(2);
    } else if words > 30 {
        complexity = complexity.saturating_add(1);
    }
    let complexity = complexity.saturating_sub(score(&text, TRIVIAL)).min(10);

    let job_classified = repo.job.is_some();
    let security = repo
        .job
        .map_or_else(|| contains_any(&text, SECURITY), |job| job.security);
    let authz = repo
        .job
        .map_or_else(|| contains_any(&text, AUTHZ), |job| job.authz);
    let schema_change = repo
        .job
        .map_or_else(|| contains_any(&text, SCHEMA), |job| job.schema_change);

    // Once the semantic classifier answered, its job type is the risk signal.
    // A translation key containing `auth` is not an authentication boundary.
    let mut risk = if job_classified { 0 } else { score(&text, RISK) };
    // The three gates the architecture names are risky by definition, whatever
    // else the wording happened to score.
    if security || authz || schema_change {
        risk = risk.max(5);
    }
    let risk = risk.min(10);

    let (blast_radius, hits) = candidates(prompt, repo);
    let signals = Signals {
        complexity,
        risk,
        architecture: contains_any(&text, ARCHITECTURE),
        security,
        authz,
        schema_change,
        bug: contains_any(&text, BUG),
        refactor: contains_any(&text, REFACTOR),
        frontend: contains_any(&text, FRONTEND),
        job_classified,
        requested_build: repo
            .job
            .map_or_else(|| text.contains("build"), |job| job.build),
        requested_test: repo
            .job
            .map_or_else(|| text.contains("test"), |job| job.test),
        requested_lint: repo
            .job
            .map_or_else(|| text.contains("lint"), |job| job.lint),
        blast_radius,
        prior_failures: repo.prior_failures,
        intent: repo.intent,
    };
    (signals, hits)
}

/// A prompt the keyword router reads as well as the classify call would: short
/// plain English, an order and not a question, a small-edit word, and nothing
/// any gate or requested action looks at. Its classify call is skipped, and
/// the sidebar shows the prompt for a title. Anything else is read.
pub fn keywords_suffice(prompt: &str) -> bool {
    let text = prompt.to_ascii_lowercase();
    let first = text.split_whitespace().next().unwrap_or_default();
    prompt.is_ascii()
        && ["fix", "rename", "correct", "update", "change", "reword", "bump"].contains(&first)
        && !prompt.contains('?')
        && text.split_whitespace().count() <= 20
        && score(&text, TRIVIAL) > 0
        && score(&text, COMPLEXITY) == 0
        && score(&text, RISK) == 0
        && ![SECURITY, AUTHZ, SCHEMA, ARCHITECTURE]
            .iter()
            .any(|words| contains_any(&text, words))
        && !contains_any(
            &text,
            &["build", "test", "lint", "how", "why", "what", "explain", "should", "could", "can you"],
        )
}

/// Turn signals into a route, its stages and its ceilings.
///
/// Rule order is not the order of the table in the architecture doc: the two
/// escalating rules are tested first, because a prompt that scores trivially
/// but touches authorisation is not a trivial task, and a prompt that has
/// already failed twice is not a candidate for the route that just failed it.
pub fn route(prompt: &str, mode: Mode, repo: &RepoSignals) -> Route {
    let (signals, candidate_paths) = classify(prompt, repo);
    let shift = mode.shift();

    let intent = signals.intent;
    // A question changes nothing, so no gate below has anything to guard.
    let (kind, stages, reason) = if intent == Some(Intent::Question) {
        (
            RouteKind::Answer,
            vec![Stage::Answer],
            "a question: one call answers it and changes no file",
        )
    } else if signals.prior_failures >= 2 {
        (
            RouteKind::Escalated,
            vec![Stage::Plan, Stage::Implement, Stage::Review],
            "this prompt has already failed twice, so it is planned and reviewed",
        )
    } else if signals.security || signals.authz {
        // A plan earns its call when the work is big enough to go wrong in its
        // shape. A one-function fix is not, and its Review still runs (§4.3.6).
        let large = signals.architecture || signals.complexity >= 7u8.saturating_add(shift);
        let mut stages = if large {
            vec![Stage::Plan, Stage::Implement, Stage::Review, Stage::Verify]
        } else {
            vec![Stage::Implement, Stage::Review, Stage::Verify]
        };
        // Tests Orteca runs itself cost no call, so they go first: a failure is
        // fixed before the deep Review reads it, and a Review of passing work
        // can spend itself on what the tests do not cover.
        if repo.checks_locally {
            let n = stages.len();
            stages.swap(n - 2, n - 1);
        }
        (
            RouteKind::Guarded,
            stages,
            if large {
                "security, authorisation or schema work this large is planned, reviewed and verified"
            } else {
                "security, authorisation or schema work is reviewed and verified; it is small enough to need no plan"
            },
        )
    } else if signals.schema_change {
        // A separate reviewer made the real schema benchmark slower and more
        // expensive than one strong Codex session. Keep deterministic tests,
        // and let the implementation session perform its own boundary review.
        let mut stages = vec![Stage::Implement];
        if repo.checks_locally {
            stages.push(Stage::Verify);
        }
        (
            RouteKind::Standard,
            stages,
            if repo.checks_locally {
                "schema work uses one strong implementation session, then Orteca runs the relevant tests"
            } else {
                "schema work uses one strong session that implements, reviews and verifies itself"
            },
        )
    } else if intent == Some(Intent::Hard)
        || (intent.is_none()
            && (signals.architecture || signals.complexity >= 7u8.saturating_add(shift)))
    {
        (
            RouteKind::Planned,
            vec![Stage::Plan, Stage::Implement, Stage::Verify],
            "architectural or complex work is planned before it is implemented",
        )
    } else if signals.risk <= 3u8.saturating_add(shift)
        && match intent {
            Some(read) => read == Intent::Easy,
            None => {
                signals.complexity <= 3u8.saturating_add(shift)
                    && signals.blast_radius <= 5usize.saturating_add(shift as usize)
                    // Narrow has to be shown, not assumed: a prompt that names no
                    // tracked file and no small-edit word is of unknown scope.
                    && (signals.blast_radius > 0
                        || score(&prompt.to_ascii_lowercase(), TRIVIAL) > 0)
            }
        }
    {
        // Tests Orteca runs itself cost no call, so the one call only edits and
        // they run after it; a failure buys the Fix, as on any checked route.
        let mut stages = vec![Stage::Implement];
        if repo.checks_locally {
            stages.push(Stage::Verify);
        }
        (
            RouteKind::ImplementOnce,
            stages,
            if repo.checks_locally {
                "small, low-risk and narrow: one call edits, then Orteca runs the tests"
            } else {
                "small, low-risk and narrow: one call that edits and verifies itself"
            },
        )
    } else {
        // The Review is what beat the plain CLI on LiftMe, and its one loss there
        // was a Standard run without one (2026-09-18). Checks Orteca runs go first,
        // as on guarded work, so the Review reads passing work.
        let mut stages = vec![Stage::Implement, Stage::Review, Stage::Verify];
        if repo.checks_locally {
            stages.swap(1, 2);
        }
        (
            RouteKind::Standard,
            stages,
            "ordinary work: implement, review and verify",
        )
    };

    let mut budget = budget_for(kind, mode);
    let mut tier_reason = "the tier this route is trusted with";
    let mut raised = false;
    // A question that did not finish was usually stopped, not too hard: a
    // higher tier would only make the retry dearer.
    if signals.prior_failures >= 1 && kind != RouteKind::Answer {
        if let Some(up) = budget.preferred_tier.up() {
            budget.preferred_tier = up;
            raised = true;
            tier_reason = "this prompt did not finish before, so it runs one tier up";
        }
    }
    // Evidence only, never exploration: a tier that stalled here is stepped
    // over, and a cheaper one is never tried on Orteca's own initiative. The
    // evidence ages out with the project's last 50 runs, which is what lets a
    // stepped-over tier be tried again.
    while repo.stalled_tiers.contains(&(kind, budget.preferred_tier)) {
        let Some(up) = budget.preferred_tier.up() else {
            break;
        };
        budget.preferred_tier = up;
        raised = true;
        tier_reason =
            "this tier stalled on 2 in 5 recent runs of this route here, so it runs one tier up";
    }
    // Short on plan allowance, the one step down, and only where the run still
    // finishes: a Review or Verify catches a miss and the Fix after it repairs
    // it. Never on a tier evidence raised, never on guarded or twice-failed
    // work, never onto a tier that stalled here.
    // ponytail: 10% of a plan window per call is a guess, not a measurement;
    // replace it with each route's measured draw once runs read limits before and after.
    const LOW_ROOM_PER_CALL: f64 = 10.0;
    let checked = stages
        .iter()
        .any(|s| matches!(s, Stage::Review | Stage::Verify));
    if let (Some(room), Some(down)) = (repo.headroom, budget.preferred_tier.down()) {
        let calls = stages.len() as u32 + u32::from(checked);
        if checked
            && !raised
            && matches!(kind, RouteKind::Standard | RouteKind::Planned)
            && !repo.stalled_tiers.contains(&(kind, down))
            && room < LOW_ROOM_PER_CALL * f64::from(calls)
        {
            budget.preferred_tier = down;
            tier_reason = "the plan limit is low, so it runs one tier down; its checks and fixes catch a miss";
        }
    }
    let preferred_providers = stages
        .iter()
        .map(|s| s.capability().preferred_provider())
        .collect();

    // A brief names a handful of paths, not a directory listing. A question
    // is about the code, so the tests paired beside it are only cost.
    let candidate_paths: Vec<String> = candidate_paths
        .into_iter()
        .filter(|p| kind != RouteKind::Answer || !is_test(p))
        .take(LISTED)
        .collect();
    let map: HashMap<&str, &FileFacts> = repo.code_map.iter().map(|(p, f)| (p.as_str(), f)).collect();
    let ours: HashSet<&str> = repo
        .code_map
        .iter()
        .flat_map(|(_, f)| f.defines.iter().filter(|s| s.kind != "method").map(|s| s.name.as_str()))
        .collect();
    let candidate_notes = candidate_paths
        .iter()
        .take(NOTED)
        .map(|p| map.get(p.as_str()).map(|f| file_note(f, &ours)).unwrap_or_default())
        .collect();
    // Earlier work counts only where it touched a file this prompt is about.
    // Words do not decide it: the first benchmark matched on them, and ordinary
    // ones (list, keep, return) pulled a pagination note into unrelated tasks.
    let mut chosen: Vec<(usize, String, String)> = Vec::new();
    for past in &repo.past {
        let shared: Vec<&str> = candidate_paths
            .iter()
            .take(NOTED)
            .filter(|p| past.files.contains(p))
            .map(String::as_str)
            .collect();
        if !shared.is_empty() {
            let why = format!("task #{}, shared files: {}", past.id, shared.join(", "));
            chosen.push((shared.len(), past.note.clone(), why));
        }
    }
    // Stable, so the newest of equal matches comes first.
    chosen.sort_by_key(|(n, ..)| std::cmp::Reverse(*n));
    chosen.truncate(3);
    let (prior_notes, prior_notes_why): (Vec<String>, Vec<String>) =
        chosen.into_iter().map(|(_, note, why)| (note, why)).unzip();
    Route {
        kind,
        mode,
        stages,
        budget,
        signals,
        reason,
        tier_reason,
        candidate_paths,
        candidate_notes,
        prior_notes,
        prior_notes_why,
        preferred_providers,
    }
}

/// How many of the listed paths the brief describes.
const NOTED: usize = 5;

/// One line on what a file holds, from the code map:
/// `class QuoteController (index, show, store); uses Quote, StoreQuoteRequest`.
/// Only names the repository defines are listed as uses: `Model` or
/// `JsonResponse` tell the agent nothing. Capped, so five lines stay far
/// under the brief's budget.
fn file_note(facts: &FileFacts, ours: &HashSet<&str>) -> String {
    const MEMBERS: usize = 8;
    const USES: usize = 8;
    fn list(names: &[&str], cap: usize) -> String {
        let mut out = names[..names.len().min(cap)].join(", ");
        if names.len() > cap {
            out.push_str(", ...");
        }
        out
    }
    let mut parts: Vec<String> = Vec::new();
    let mut functions: Vec<&str> = Vec::new();
    // Methods follow their type in line order.
    let mut current: Option<(String, Vec<&str>)> = None;
    for symbol in &facts.defines {
        match (symbol.kind.as_str(), current.as_mut()) {
            ("method", Some((_, members))) => members.push(&symbol.name),
            ("method" | "function", _) => functions.push(&symbol.name),
            (kind, _) => {
                parts.extend(current.take().map(|(head, members)| typed(head, &members)));
                current = Some((format!("{kind} {}", symbol.name), Vec::new()));
            }
        }
    }
    fn typed(head: String, members: &[&str]) -> String {
        if members.is_empty() { head } else { format!("{head} ({})", list(members, MEMBERS)) }
    }
    parts.extend(current.map(|(head, members)| typed(head, &members)));
    if !functions.is_empty() {
        parts.push(format!("functions {}", list(&functions, MEMBERS)));
    }
    let uses: Vec<&str> = facts
        .uses
        .iter()
        .map(String::as_str)
        .filter(|u| ours.contains(u))
        .collect();
    if !uses.is_empty() {
        parts.push(format!("uses {}", list(&uses, USES)));
    }
    parts.join("; ")
}

/// Tiers per route.
fn budget_for(kind: RouteKind, mode: Mode) -> ExecutionBudget {
    let efficient = mode == Mode::Efficient;
    ExecutionBudget {
        preferred_tier: match (kind, efficient) {
            (RouteKind::Answer | RouteKind::ImplementOnce, _) | (RouteKind::Standard, true) => {
                Tier::Cheapest
            }
            // Guarded builds on standard and reviews on deep: four deep calls
            // cost 6x the plain CLI for a one-function fix (§4.3.6).
            (RouteKind::Standard, false)
            | (RouteKind::Planned, true)
            | (RouteKind::Escalated, true)
            | (RouteKind::Guarded, _) => Tier::Standard,
            _ => Tier::Deep,
        },
        // Standard reviews on deep too: Sonnet missed a seeded regression Opus
        // medium caught, for no saving (§4.3.6). Schema Standard has no Review.
        review_tier: matches!(kind, RouteKind::Guarded | RouteKind::Standard).then_some(Tier::Deep),
    }
}

/// What one finished stage hands to the next.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StageNote {
    pub stage: Stage,
    /// The provider's own closing words. Forwarded, never parsed.
    pub summary: String,
    /// Present only when the provider returned a structured artifact that
    /// passed `artifact_is_valid`.
    pub artifact: Option<serde_json::Value>,
    /// The model and reasoning effort Orteca asked for; none for a check
    /// Orteca ran itself.
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub effort: Option<String>,
    /// Wall time of the stage, Orteca's own checks included.
    #[serde(default)]
    pub duration_ms: Option<u64>,
}

/// How much of one failing check's output a Fix brief carries.
// ponytail: 4 KB a check, no total cap; add one if a run ever fails many
// checks at once.
const CHECK_OUTPUT: usize = 4096;

/// The nearest character boundary at or below `at`, or above it when `up`.
fn boundary(text: &str, at: usize, up: bool) -> usize {
    let mut at = at.min(text.len());
    while !text.is_char_boundary(at) {
        at = if up { at + 1 } else { at - 1 };
    }
    at
}

/// A long string's head and tail, with a line naming what was cut. Both ends,
/// because a compiler names the first error and a test suite the last.
fn clip(text: &str, limit: usize) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    let head = boundary(text, limit / 2, false);
    let tail = boundary(text, text.len() - limit / 2, true);
    format!("{}
[{} bytes cut]
{}", &text[..head], tail - head, &text[tail..])
}

/// An unrecognised provider line, cut to what diagnosing it needs. The head
/// names the type and the tail shows how it ended; a Read result or a pasted
/// image's base64 in between says nothing either does not.
pub fn clip_event(line: &str) -> String {
    const UNKNOWN_EVENT: usize = 2048;
    clip(line, UNKNOWN_EVENT)
}

/// One stage's artifact as the next one should see it.
///
/// A Verify or Fix artifact carries every check's whole stdout, and a passing
/// suite's is the bulk of it. The Fix is bought to repair what failed, so a
/// passing check hands on its command and its verdict - enough to say what
/// must not break - and not its output, and a failing one is cut to
/// `CHECK_OUTPUT`. The whole log is in the event log either way.
fn forwarded(stage: Stage, artifact: &serde_json::Value) -> String {
    if !matches!(stage, Stage::Verify | Stage::Fix) {
        return artifact.to_string();
    }
    let mut artifact = artifact.clone();
    if let Some(checks) = artifact.get_mut("checks").and_then(serde_json::Value::as_array_mut) {
        for check in checks {
            let passed = check["passed"] == true;
            let Some(output) = check["output"].as_str() else {
                continue;
            };
            let kept = if passed { String::new() } else { clip(output, CHECK_OUTPUT) };
            check["output"] = serde_json::Value::String(kept);
        }
    }
    artifact.to_string()
}

/// The prompt one stage is given.
///
/// A brief names paths and states the contract. It never pastes file contents,
/// never dumps repository status, and never puts a multi-stage checklist in
/// front of a one-call task - all three are ways of paying twice for context
/// the agent would have fetched itself.
pub fn brief(
    route: &Route,
    stage: Stage,
    prompt: &str,
    constraints: &[String],
    carried: &[StageNote],
) -> String {
    let mut out = String::new();

    match stage {
        Stage::Plan => out
            .push_str("Plan this work. Do not edit any file. Return only the structured plan.\n\n"),
        Stage::Review => {
            out.push_str(
                "Review the change that is already in the working tree against the task below. \
                 Do not edit any file. Return only the structured review. Check it against \
                 every rule the task states: a rule that is missing or only partly met is a \
                 finding. Ask for changes only for a high or medium finding; report low ones, \
                 which do not hold the work up. Say what each finding requires and what it \
                 must not break; do not write the patch or dictate an implementation.",
            );
            // The latest check, not any: a pass before a later fix says nothing
            // about the tree as it is now.
            let tested = carried
                .iter()
                .rev()
                .find(|n| matches!(n.stage, Stage::Verify | Stage::Fix))
                .is_some_and(|n| {
                    n.artifact
                        .as_ref()
                        .is_some_and(|a| stage_passed(n.stage, a))
                });
            let running = route.verifies_before_review()
                && !carried.iter().any(|n| matches!(n.stage, Stage::Verify | Stage::Fix));
            if tested {
                out.push_str(
                    " The repository's tests already pass on it; spend the review on the \
                     inputs and paths they do not cover.",
                );
            } else if running {
                // Orteca runs them beside this Review.
                out.push_str(
                    " The repository's tests are running on it now; spend the review on the \
                     inputs and paths they do not cover.",
                );
            }
            out.push_str("\n\n");
        }
        Stage::Verify => out.push_str(
            "Verify the change that is already in the working tree. Run the focused checks \
             that prove it and change nothing else. Return only the structured result: every \
             check you ran, whether it passed, and what it printed. `pass` means at least one \
             check ran and every check passed.\n\n",
        ),
        Stage::Fix => out.push_str(
            "An earlier stage of this task did not pass; what it found is below. Fix that and \
             nothing beyond it, then run the focused checks that prove the fix. Return only \
             the structured result: every check you ran, whether it passed, and what it \
             printed. `pass` means at least one check ran and every check passed. The check \
             that failed runs again after this call.\n\n",
        ),
        Stage::Answer => out.push_str(
            "Answer the question below about this project. Read what you need, change no \
             file, and reply in the language it was asked in. Stop reading as soon as you \
             can answer; do not search again to double-check. Explain it like to a \
             five-year-old: small words, short sentences, only what matters, a few short \
             paragraphs at most. Lead with the answer. Plain text only: no markdown, no \
             headings, no bold, no [[links]]; name a file only when the reader needs it.\n\n",
        ),
        Stage::Implement => {}
    }

    out.push_str("Task:\n");
    out.push_str(prompt.trim());
    out.push('\n');

    if !route.candidate_paths.is_empty() {
        out.push_str(if stage == Stage::Answer {
            // Naming the files as the user's makes the model review them
            // ("this file is about...") instead of answering. Saying what not to
            // do primed the same thing, so this only says who sees what.
            "\nOrteca matched these paths to the question's words; the user has not seen \
             this list, so the answer stands on its own unless the question names a file. \
             Open one only if it helps:\n"
        } else {
            "\nStart here. These tracked paths match the task, most likely first; open the \
             ones you need directly, with no search first, and ignore the rest:\n"
        });
        for (i, path) in route.candidate_paths.iter().enumerate() {
            out.push_str("- ");
            out.push_str(path);
            if let Some(note) = route.candidate_notes.get(i).filter(|n| !n.is_empty()) {
                out.push_str(": ");
                out.push_str(note);
            }
            out.push('\n');
        }
    }

    // Implement only: later stages read the tree and the task, and a note on
    // old work would be paid for again in each of them.
    if stage == Stage::Implement && !route.prior_notes.is_empty() {
        out.push_str(
            "\nEarlier finished work in this project that may bear on the task, from \
             Orteca's records. The working tree is the truth if it disagrees:\n",
        );
        for note in &route.prior_notes {
            out.push_str("- ");
            out.push_str(note);
            out.push('\n');
        }
    }

    if !constraints.is_empty() {
        // Every later stage carries every instruction the user has given. An
        // instruction never silently expires.
        out.push_str("\nStanding instructions from the user, all of which still apply:\n");
        for c in constraints {
            out.push_str("- ");
            out.push_str(c.trim());
            out.push('\n');
        }
    }

    // A new process can inspect the working tree. Carry only information it
    // cannot recover there: the Plan into Implement or the failed contract
    // that triggered Fix.
    let handoff: Vec<&StageNote> = match stage {
        Stage::Implement => carried
            .iter()
            .rev()
            .find(|n| n.stage == Stage::Plan)
            .into_iter()
            .collect(),
        Stage::Fix => {
            // A Review and a Verify that ran side by side each hand on what
            // they found.
            let mut failed: Vec<&StageNote> = carried
                .iter()
                .rev()
                .take_while(|n| matches!(n.stage, Stage::Review | Stage::Verify))
                .filter(|n| !n.artifact.as_ref().is_some_and(|a| stage_passed(n.stage, a)))
                .collect();
            if failed.is_empty() {
                failed.extend(
                    carried
                        .iter()
                        .rev()
                        .find(|n| matches!(n.stage, Stage::Review | Stage::Verify | Stage::Fix)),
                );
            }
            failed.reverse();
            failed
        }
        Stage::Plan | Stage::Review | Stage::Verify | Stage::Answer => Vec::new(),
    };
    for note in handoff {
        match &note.artifact {
            Some(artifact) => {
                out.push_str("\nValidated ");
                out.push_str(note.stage.name());
                out.push_str(" artifact:\n");
                out.push_str(&forwarded(note.stage, artifact));
                out.push('\n');
            }
            // No artifact came back, so nothing here claims one did. The
            // stage's own words are forwarded verbatim and labelled unvalidated;
            // they are never parsed into the fields a schema would have filled.
            None if !note.summary.trim().is_empty() => {
                out.push_str("\nThe ");
                out.push_str(note.stage.name());
                out.push_str(
                    " stage returned no structured artifact. Its closing words, \
                     unvalidated, were:\n",
                );
                out.push_str(note.summary.trim());
                out.push('\n');
            }
            None => {}
        }
    }

    if stage == Stage::Implement {
        if route.signals.schema_change {
            out.push_str(
                "Before finishing, review the completed diff for strict input types, \
                 migration/schema agreement, and queries that bypass useful indexes. \
                 Correct any issue you find in this same session.\n",
            );
        }
        if route.is_single_call() {
            out.push_str(
                "This is the only agent call for this task. Make the change. If it changes \
                 behaviour, run one focused check that proves it works and stop as soon as \
                 it passes. A change with nothing to run - a comment, docs, wording - needs \
                 no check: stop once the edit is made. Do not broaden the task, refactor \
                 around it, or run the whole suite. No other check follows this call, so \
                 end by saying in one sentence what you checked and what you did not.\n",
            );
        } else if route.stages.contains(&Stage::Verify) {
            out.push_str(
                "Make the change described above and nothing beyond it. If it changes \
                 behaviour, add or update tests that pin every rule the task states, \
                 edge values included: the later checks only catch what a test covers. \
                 A later stage runs the repository's checks and any build or lint action \
                 the user requested, directly through Orteca. Do not run those commands \
                 yourself; stop when the change is complete.\n",
            );
        } else {
            out.push_str(
                "Make the change described above and nothing beyond it. A later stage \
                 checks the work, so stop when the change is complete.\n",
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const REPO: &[&str] = &[
        "src/main.rs",
        "src/run.rs",
        "src/routing.rs",
        "src/store.rs",
        "src/views/Project.vue",
        "docs/architecture.md",
        "README.md",
    ];

    fn repo(paths: &[&str]) -> RepoSignals {
        RepoSignals {
            tracked_paths: paths.iter().map(|p| (*p).to_string()).collect(),
            ..Default::default()
        }
    }

    /// Name beats substring, recent history breaks what depth would decide, and
    /// a test is listed beside the file it tests without widening the blast
    /// radius.
    #[test]
    fn candidates_are_ranked_by_name_recency_and_paired_tests() {
        let signals = RepoSignals {
            tracked_paths: [
                "slugs.txt",
                "docs/slug-notes.md",
                "slugify.rs",
                "src/deep/slug.rs",
            ]
            .map(String::from)
            .to_vec(),
            recent_paths: vec!["docs/slug-notes.md".into()],
            ..Default::default()
        };
        let r = route("tidy the slug code", Mode::Balanced, &signals);
        assert_eq!(
            r.candidate_paths,
            [
                "docs/slug-notes.md",
                "src/deep/slug.rs",
                "slugs.txt",
                "slugify.rs"
            ]
        );

        let r = balanced(
            "fix slug.rs",
            &["src/deep/slug.rs", "tests/test_slug.py", "src/other.rs"],
        );
        assert_eq!(
            r.candidate_paths,
            ["src/deep/slug.rs", "tests/test_slug.py"]
        );
        assert_eq!(
            r.signals.blast_radius, 1,
            "a paired test widened the blast radius"
        );
        // Named by the prompt too: still paired, still not blast radius.
        let r = balanced("fix slug tests", &["src/slug.rs", "tests/test_slug.py"]);
        assert_eq!(
            r.signals.blast_radius, 1,
            "a named paired test widened the blast radius"
        );
    }

    #[test]
    fn the_code_map_finds_files_the_prompt_never_names() {
        let facts = |defines: &[(&str, &str)], uses: &[&str]| FileFacts {
            defines: defines
                .iter()
                .map(|(name, kind)| crate::codemap::Symbol { name: name.to_string(), kind: kind.to_string(), line: 1 })
                .collect(),
            uses: uses.iter().map(|u| u.to_string()).collect(),
        };
        let map = vec![
            ("routes/api.php".to_string(), facts(&[], &["QuoteController"])),
            (
                "app/Http/Controllers/Api/OfferController.php".to_string(),
                facts(&[("OfferController", "class"), ("store", "method")], &["Quote"]),
            ),
            ("app/Models/Quote.php".to_string(), facts(&[("Quote", "class")], &[])),
            ("app/Models/Invoice.php".to_string(), facts(&[("Invoice", "class")], &[])),
            (
                "app/Http/Controllers/QuoteController.php".to_string(),
                facts(&[("QuoteController", "class")], &["Quote"]),
            ),
            ("tests/Feature/EditingTest.php".to_string(), facts(&[], &["url:api/quotes/*"])),
        ];
        let signals = RepoSignals {
            tracked_paths: map.iter().map(|(p, _)| p.clone()).chain(["resources/js/stores/chat.js".to_string()]).collect(),
            code_map: map,
            ..Default::default()
        };
        let r = route("Customers can edit quotes they sent", Mode::Balanced, &signals);
        let listed = &r.candidate_paths;
        assert_eq!(listed[..2], ["app/Models/Quote.php", "app/Http/Controllers/QuoteController.php"]);
        assert!(listed.contains(&"routes/api.php".to_string()), "linked to a seed: {listed:?}");
        assert!(listed.contains(&"app/Http/Controllers/Api/OfferController.php".to_string()));
        assert!(!listed.contains(&"app/Models/Invoice.php".to_string()));
        assert_eq!(r.signals.blast_radius, 2, "only path matches widen the blast radius");

        let r = route("PATCH /api/quotes/{quote} must refuse a priced one", Mode::Balanced, &signals);
        assert!(r.candidate_paths.contains(&"tests/Feature/EditingTest.php".to_string()), "calls the endpoint");

        let r = route("Add an archive action to resources/js/stores/chat.js for a quote", Mode::Balanced, &signals);
        assert_eq!(r.candidate_paths[0], "resources/js/stores/chat.js", "a named path goes first");
        assert_eq!(r.candidate_notes[0], "", "an unmapped file has no note");
    }

    #[test]
    fn the_brief_says_what_the_first_files_hold() {
        let symbol = |name: &str, kind: &str| crate::codemap::Symbol { name: name.into(), kind: kind.into(), line: 1 };
        let controller = FileFacts {
            defines: vec![
                symbol("QuoteController", "class"),
                symbol("index", "method"),
                symbol("show", "method"),
                symbol("Status", "enum"),
                symbol("helper", "function"),
            ],
            uses: ["Quote", "StoreQuoteRequest", "url:api/quotes", "lang/*/shop.php"].map(String::from).to_vec(),
        };
        let ours: HashSet<&str> = ["Quote", "StoreQuoteRequest"].into();
        assert_eq!(
            file_note(&controller, &ours),
            "class QuoteController (index, show); enum Status; functions helper; uses Quote, StoreQuoteRequest"
        );
        let many = FileFacts { defines: vec![], uses: (0..10).map(|i| format!("U{i}")).collect() };
        let names: Vec<String> = (0..10).map(|i| format!("U{i}")).collect();
        let ours: HashSet<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(file_note(&many, &ours), "uses U0, U1, U2, U3, U4, U5, U6, U7, ...");
        assert_eq!(file_note(&many, &HashSet::new()), "", "framework names are not listed");

        let path = "app/Http/Controllers/QuoteController.php".to_string();
        let signals = RepoSignals {
            tracked_paths: vec![path.clone()],
            code_map: vec![(path, controller)],
            ..Default::default()
        };
        let r = route("fix the quote controller", Mode::Balanced, &signals);
        let text = brief(&r, r.stages[0], "fix the quote controller", &[], &[]);
        assert!(
            text.contains("- app/Http/Controllers/QuoteController.php: class QuoteController (index, show); enum Status; functions helper\n"),
            "{text}"
        );
    }

    fn balanced(prompt: &str, paths: &[&str]) -> Route {
        route(prompt, Mode::Balanced, &repo(paths))
    }

    /// Ordinary work is reviewed on deep, after the checks Orteca runs itself.
    #[test]
    fn standard_work_is_reviewed_after_local_checks() {
        let medium = |checks_locally| RepoSignals {
            intent: Some(Intent::Medium),
            checks_locally,
            ..RepoSignals::default()
        };
        let r = route("make the header bold", Mode::Efficient, &medium(true));
        assert_eq!(r.kind, RouteKind::Standard);
        assert_eq!(r.stages, [Stage::Implement, Stage::Verify, Stage::Review]);
        assert_eq!(r.budget.review_tier, Some(Tier::Deep));
        assert_eq!(r.review_model(ProviderId::Claude).map(|m| m.effort), Some("medium"));
        let r = route("make the header bold", Mode::Balanced, &medium(false));
        assert_eq!(r.stages, [Stage::Implement, Stage::Review, Stage::Verify]);
    }

    /// Short on allowance, a checked route runs one tier down. Never where
    /// evidence raised the tier, never on
    /// guarded work, never onto a tier that stalled, never without a check.
    /// The bench that bought the deep Review tier caught its seeded regression
    /// with Opus *medium*; shipping high was effort nothing measured asked for.
    #[test]
    fn a_claude_review_reasons_at_medium_in_every_mode() {
        for mode in [Mode::Balanced, Mode::Efficient] {
            let r = route("make the header bold", mode, &repo(REPO));
            let Some(review) = r.review_model(ProviderId::Claude) else {
                continue;
            };
            assert_eq!(review.model, "opus", "{mode:?}");
            assert_eq!(review.effort, "medium", "{mode:?}");
        }
    }

    /// A reviewer that writes the patch spends the route's dearest output
    /// tokens on code the fixing agent then copies without reading the repo.
    #[test]
    fn a_review_is_asked_for_the_requirement_and_not_the_patch() {
        assert!(REVIEW_SCHEMA.contains("do not write the patch"), "{REVIEW_SCHEMA}");
        let brief = brief(
            &balanced("make the header bold", REPO),
            Stage::Review,
            "make the header bold",
            &[],
            &[],
        );
        assert!(brief.contains("do not write the patch"), "{brief}");
    }

    #[test]
    fn a_low_plan_limit_drops_a_checked_route_one_tier() {
        let low = |room: f64| RepoSignals {
            headroom: Some(room),
            ..repo(REPO)
        };
        let r = route("make the header bold", Mode::Balanced, &low(20.0));
        assert_eq!(r.kind, RouteKind::Standard);
        assert_eq!(r.budget.preferred_tier, Tier::Cheapest);
        assert!(r.tier_reason.contains("one tier down"), "{}", r.tier_reason);

        assert_eq!(
            route("make the header bold", Mode::Balanced, &low(60.0))
                .budget
                .preferred_tier,
            Tier::Standard,
            "enough room"
        );
        assert_eq!(
            balanced("make the header bold", REPO).budget.preferred_tier,
            Tier::Standard,
            "an unread limit is not a low one"
        );

        let mut raised = low(5.0);
        raised.prior_failures = 1;
        assert_eq!(
            route("make the header bold", Mode::Balanced, &raised)
                .budget
                .preferred_tier,
            Tier::Deep,
            "evidence outranks allowance"
        );

        let mut stalled = low(5.0);
        stalled.stalled_tiers = vec![(RouteKind::Standard, Tier::Cheapest)];
        assert_eq!(
            route("make the header bold", Mode::Balanced, &stalled)
                .budget
                .preferred_tier,
            Tier::Standard,
            "never onto a tier that stalled"
        );

        let guarded = route("fix the password check", Mode::Balanced, &low(1.0));
        assert_eq!(
            (
                guarded.kind,
                guarded.budget.preferred_tier,
                guarded.budget.review_tier
            ),
            (RouteKind::Guarded, Tier::Standard, Some(Tier::Deep)),
            "guarded work keeps its tiers"
        );
        assert_eq!(
            route("fix the typo", Mode::Balanced, &low(1.0))
                .budget
                .preferred_tier,
            Tier::Cheapest,
            "one call has no check to catch a miss"
        );
    }

    /// The headline of the milestone: a small, low-risk, narrow task costs one
    /// agent call, and that call verifies itself.
    #[test]
    fn only_a_plain_small_order_skips_the_classify_call() {
        assert!(keywords_suffice("fix the typo in README.md"));
        assert!(keywords_suffice("Rename getUser to fetchUser in src/api.ts"));
        for read in [
            "fix the typo in README.md?",
            "why is there a typo in the readme",
            "fix the typo in the login password screen",
            "rename the users table column",
            "fix the typo and run the tests",
            "fix the typo in the header component and redesign the page",
            "corrige la faute de frappe dans le readme",
            "make the header bold",
            "corrige l'erreur — typo dans le README",
        ] {
            assert!(!keywords_suffice(read), "{read}");
        }
    }

    #[test]
    fn a_trivial_task_is_one_implement_call_with_no_plan_and_no_review() {
        let r = balanced("Fix the typo in the README heading", REPO);
        assert_eq!(r.kind, RouteKind::ImplementOnce);
        assert_eq!(r.stages, [Stage::Implement]);
        assert!(r.is_single_call());
        assert!(
            !r.stages.contains(&Stage::Plan),
            "a typo does not need a plan"
        );
        assert!(
            !r.stages.contains(&Stage::Review),
            "a typo does not need a review"
        );

        let brief = brief(
            &r,
            Stage::Implement,
            "Fix the typo in the README heading",
            &[],
            &[],
        );
        assert!(
            brief.contains("one focused check"),
            "verification happens inside the single call"
        );
        // What §12 forbids: a generic multi-stage checklist on a small task.
        for word in ["Plan stage", "Review stage", "plan artifact"] {
            assert!(
                !brief.contains(word),
                "a one-call brief must not carry `{word}`"
            );
        }
    }

    /// When Orteca can run the tests, the one call only edits: it is told not
    /// to check, and Orteca's own Verify follows it.
    #[test]
    fn a_trivial_task_leaves_its_tests_to_orteca_when_it_can_run_them() {
        let local = RepoSignals {
            checks_locally: true,
            ..repo(REPO)
        };
        let r = route("Fix the typo in the README heading", Mode::Balanced, &local);
        assert_eq!(
            (r.kind, r.stages.as_slice()),
            (
                RouteKind::ImplementOnce,
                [Stage::Implement, Stage::Verify].as_slice()
            )
        );
        assert!(!r.is_single_call());
        let text = brief(
            &r,
            Stage::Implement,
            "Fix the typo in the README heading",
            &[],
            &[],
        );
        assert!(
            text.contains("Do not run those commands")
                && text.contains("pin every rule")
                && !text.contains("one focused check"),
            "{text}"
        );
    }

    #[test]
    fn semantic_job_classification_overrides_auth_words_in_ui_copy() {
        let local = RepoSignals {
            checks_locally: true,
            intent: Some(Intent::Easy),
            job: Some(crate::intent::Job {
                build: true,
                ..Default::default()
            }),
            ..repo(&["resources/js/locales/en.js", "resources/js/pages/Login.vue"])
        };
        let r = route(
            "fix auth.rememberMe on the login page and build the files",
            Mode::Balanced,
            &local,
        );
        assert_eq!(r.kind, RouteKind::ImplementOnce);
        assert_eq!(r.stages, [Stage::Implement, Stage::Verify]);
        assert!(r.signals.job_classified && r.signals.requested_build);
        assert!(!r.signals.authz);
    }

    #[test]
    fn security_and_authz_work_is_reviewed_on_deep_and_verified() {
        for prompt in [
            "sanitize the user input to close the injection vulnerability",
            "add an authorization check before the delete endpoint",
        ] {
            let r = balanced(prompt, REPO);
            assert_eq!(r.kind, RouteKind::Guarded, "`{prompt}` was not guarded");
            assert_eq!(
                r.stages,
                [Stage::Implement, Stage::Review, Stage::Verify],
                "`{prompt}` is small and needs no plan"
            );
            assert_eq!(
                (r.budget.preferred_tier, r.budget.review_tier),
                (Tier::Standard, Some(Tier::Deep))
            );
            assert!(
                r.signals.risk >= 5,
                "`{prompt}` scored risk {}",
                r.signals.risk
            );
        }
        let large = balanced(
            "redesign the authorization subsystem so every endpoint checks it",
            REPO,
        );
        assert_eq!(
            large.stages,
            [Stage::Plan, Stage::Implement, Stage::Review, Stage::Verify]
        );

        // Tests Orteca runs itself go before the Review.
        let local = RepoSignals {
            checks_locally: true,
            ..repo(REPO)
        };
        let r = route(
            "add an authorization check before the delete endpoint",
            Mode::Balanced,
            &local,
        );
        assert_eq!(r.stages, [Stage::Implement, Stage::Verify, Stage::Review]);
        assert!(
            r.verifies_before_review()
                && !balanced(
                    "add an authorization check before the delete endpoint",
                    REPO
                )
                .verifies_before_review()
        );
        let r = route(
            "redesign the authorization subsystem so every endpoint checks it",
            Mode::Balanced,
            &local,
        );
        assert_eq!(
            r.stages,
            [Stage::Plan, Stage::Implement, Stage::Verify, Stage::Review]
        );

        // Balanced reviews on deep too - the deep model, at the effort the
        // seeded review benchmark actually passed on.
        let claude = ProviderId::Claude;
        assert_eq!(
            r.review_model(claude).map(|c| (c.model, c.effort)),
            Some((Tier::Deep.model(claude).model, "medium"))
        );
        let lean = route(
            "add an authorization check before the delete endpoint",
            Mode::Efficient,
            &local,
        );
        assert_eq!(
            lean.review_model(claude).map(|c| (c.model, c.effort)),
            Some(("opus", "medium"))
        );
        assert_eq!(
            lean.review_model(ProviderId::Codex)
                .map(|c| (c.model, c.effort)),
            Some(("gpt-5.6-terra", "high"))
        );
        assert_eq!(
            r.review_model(ProviderId::Codex)
                .map(|c| (c.model, c.effort)),
            Some(("gpt-5.6-sol", "high"))
        );
    }

    #[test]
    fn schema_work_uses_one_strong_codex_session_and_local_verify() {
        let prompt = "write a migration that adds a column to the tasks table";
        let balanced_route = balanced(prompt, REPO);
        assert_eq!(
            (balanced_route.kind, balanced_route.stages.as_slice()),
            (RouteKind::Standard, [Stage::Implement].as_slice())
        );
        assert_eq!(
            balanced_route
                .work_model(ProviderId::Codex)
                .map(|choice| (choice.model, choice.effort)),
            Some(("gpt-5.6-sol", "medium"))
        );
        assert!(brief(&balanced_route, Stage::Implement, prompt, &[], &[])
            .contains("migration/schema agreement"));

        let local = RepoSignals {
            checks_locally: true,
            ..repo(REPO)
        };
        let efficient = route(prompt, Mode::Efficient, &local);
        assert_eq!(efficient.stages, [Stage::Implement, Stage::Verify]);
        assert_eq!(
            efficient
                .work_model(ProviderId::Codex)
                .map(|choice| (choice.model, choice.effort)),
            Some(("gpt-5.6-sol", "medium"))
        );
    }

    /// A prompt can read as small and still be dangerous. The gate wins.
    #[test]
    fn a_small_sounding_security_change_is_never_the_one_call_route() {
        let r = balanced("rename the password field", REPO);
        assert_ne!(r.kind, RouteKind::ImplementOnce);
        assert!(r.stages.contains(&Stage::Review));
    }

    #[test]
    fn architectural_or_complex_work_is_planned_first() {
        let r = balanced(
            "redesign the storage subsystem so it can be swapped out",
            REPO,
        );
        assert_eq!(r.kind, RouteKind::Planned);
        assert_eq!(r.stages[0], Stage::Plan);
        assert!(r.signals.architecture);

        let lean = route(
            "redesign the storage subsystem so it can be swapped out",
            Mode::Efficient,
            &repo(REPO),
        );
        assert_eq!(
            lean.plan_model(ProviderId::Codex)
                .map(|c| (c.model, c.effort)),
            Some(("gpt-5.6-terra", "low"))
        );
        assert_eq!(
            r.plan_model(ProviderId::Codex),
            None,
            "Balanced keeps the tier default"
        );
    }

    #[test]
    fn a_prompt_that_failed_twice_is_escalated_rather_than_repeated() {
        let mut signals = repo(REPO);
        signals.prior_failures = 1;
        assert_eq!(
            route("fix the typo", Mode::Balanced, &signals).kind,
            RouteKind::ImplementOnce
        );
        signals.prior_failures = 2;
        let escalated = route("fix the typo", Mode::Balanced, &signals);
        assert_eq!(escalated.kind, RouteKind::Escalated);
        assert_eq!(
            escalated.stages,
            [Stage::Plan, Stage::Implement, Stage::Review]
        );
    }

    /// The small model's reading picks the route in any language, and the
    /// keyword gates still escalate what it calls easy or medium.
    #[test]
    fn the_read_intent_picks_the_route_and_gates_still_escalate() {
        let read = |intent| RepoSignals {
            intent: Some(intent),
            ..repo(REPO)
        };
        let question = route(
            "Ja lietotājs atcēla abonementu, vai man viņam rakstīt?",
            Mode::Balanced,
            &read(Intent::Question),
        );
        assert_eq!(question.kind, RouteKind::Answer);
        assert_eq!(question.stages, [Stage::Answer]);
        assert!(!Stage::Answer.writes());

        // A question lists no tests and a failed one is not retried a tier up.
        let asked = RepoSignals {
            intent: Some(Intent::Question),
            prior_failures: 1,
            tracked_paths: vec!["app/Winback.php".into(), "tests/WinbackTest.php".into()],
            ..repo(REPO)
        };
        let retried = route("how does winback work?", Mode::Balanced, &asked);
        assert_eq!(retried.candidate_paths, ["app/Winback.php"]);
        assert_eq!(
            retried.budget.preferred_tier,
            route("how does winback work?", Mode::Balanced, &RepoSignals { prior_failures: 0, ..asked })
                .budget
                .preferred_tier
        );

        let kind = |prompt, intent| route(prompt, Mode::Balanced, &read(intent)).kind;
        assert_eq!(kind("pievieno pogu", Intent::Easy), RouteKind::ImplementOnce);
        assert_eq!(kind("fix the typo", Intent::Medium), RouteKind::Standard);
        assert_eq!(kind("fix the typo", Intent::Hard), RouteKind::Planned);
        assert_eq!(
            kind("add an authorization check to delete", Intent::Easy),
            RouteKind::Guarded
        );

        let mut failed = read(Intent::Easy);
        failed.prior_failures = 2;
        assert_eq!(
            route("fix the typo", Mode::Balanced, &failed).kind,
            RouteKind::Escalated
        );
    }

    /// Efficient is the same table read two points further along, so work that
    /// Balanced plans, Efficient may implement directly. It can never add work.
    #[test]
    fn efficient_mode_shifts_thresholds_and_never_adds_a_stage() {
        for prompt in [
            "fix the typo",
            "add a button to the settings screen and wire it up",
            "integrate a new protocol module with async performance work",
            "add an authorization check before the delete endpoint",
        ] {
            let lean = route(prompt, Mode::Efficient, &repo(REPO));
            let full = route(prompt, Mode::Balanced, &repo(REPO));
            assert!(
                lean.stages.len() <= full.stages.len(),
                "`{prompt}` grew from {} to {} stages in Efficient mode",
                full.stages.len(),
                lean.stages.len()
            );
        }
    }

    /// The gates hold in both modes: cheaper must never mean less careful about
    /// authorisation, security or the schema.
    #[test]
    fn efficient_mode_does_not_relax_the_risk_gates() {
        let r = route(
            "add an authorization check before the delete endpoint",
            Mode::Efficient,
            &repo(REPO),
        );
        assert_eq!(r.kind, RouteKind::Guarded);
        assert!(r.stages.contains(&Stage::Review));
    }

    /// The tier moves up on evidence - this prompt failing, or this tier
    /// stalling on this route here - and never down, and never past Deep.
    #[test]
    fn a_tier_only_steps_up_on_evidence_and_deep_is_the_ceiling() {
        assert_eq!(
            balanced("fix the typo", REPO).budget.preferred_tier,
            Tier::Cheapest
        );
        let standard = balanced("make the header bold", REPO);
        assert_eq!(standard.budget.preferred_tier, Tier::Standard);
        let fix = brief(&standard, Stage::Fix, "make the header bold", &[], &[]);
        assert!(fix.contains("did not pass") && fix.contains("runs again after this call"));
        assert!(
            !fix.contains("agent call"),
            "a brief carries no call ceiling: {fix}"
        );

        let mut signals = repo(REPO);
        signals.prior_failures = 1;
        assert_eq!(
            route("fix the typo", Mode::Balanced, &signals)
                .budget
                .preferred_tier,
            Tier::Standard
        );

        let mut signals = repo(REPO);
        signals.stalled_tiers = vec![
            (RouteKind::ImplementOnce, Tier::Cheapest),
            (RouteKind::ImplementOnce, Tier::Standard),
        ];
        let r = route("fix the typo", Mode::Balanced, &signals);
        assert_eq!(r.budget.preferred_tier, Tier::Deep);
        assert!(r.tier_reason.contains("stalled"));
        // Evidence about another route kind says nothing about this one.
        signals.stalled_tiers = vec![(RouteKind::Standard, Tier::Cheapest)];
        assert_eq!(
            route("fix the typo", Mode::Balanced, &signals)
                .budget
                .preferred_tier,
            Tier::Cheapest
        );
        signals.stalled_tiers = vec![(RouteKind::Planned, Tier::Deep)];
        assert_eq!(
            route("redesign the storage subsystem", Mode::Balanced, &signals)
                .budget
                .preferred_tier,
            Tier::Deep
        );
    }

    #[test]
    fn a_wide_prompt_leaves_the_one_call_route_even_when_it_scores_low() {
        let narrow = repo(&["src/widget/one_widget.rs", "docs/architecture.md"]);
        let prompt = "rename widget";
        assert_eq!(
            route(prompt, Mode::Balanced, &narrow).kind,
            RouteKind::ImplementOnce
        );

        let broad = RepoSignals {
            tracked_paths: (0..40)
                .map(|i| format!("src/widget/{i}_widget.rs"))
                .collect(),
            ..Default::default()
        };
        let r = route(prompt, Mode::Balanced, &broad);
        assert!(
            r.signals.blast_radius > 5,
            "blast radius was {}",
            r.signals.blast_radius
        );
        assert_ne!(
            r.kind,
            RouteKind::ImplementOnce,
            "a prompt touching 40 files is not narrow"
        );
    }

    #[test]
    fn classification_is_pure_and_repeatable() {
        let prompt = "add an authorization check before the delete endpoint";
        assert_eq!(
            route(prompt, Mode::Balanced, &repo(REPO)),
            route(prompt, Mode::Balanced, &repo(REPO))
        );
    }

    #[test]
    fn a_fix_carries_the_failure_and_not_the_suite_that_passed() {
        let r = balanced("make the header bold", REPO);
        let noise = "x".repeat(CHECK_OUTPUT * 2);
        let failed = StageNote {
            stage: Stage::Verify,
            model: None,
            effort: None,
            duration_ms: None,
            summary: "failed check".into(),
            artifact: Some(serde_json::json!({
                "checks": [
                    {"command": "cargo test", "passed": true, "output": "passing-suite-noise"},
                    {"command": "npm test", "passed": false,
                     "output": format!("first-error\n{noise}\nlast-error")},
                ],
                "verdict": "fail"
            })),
        };
        let fix = brief(&r, Stage::Fix, "task", &[], &[failed]);
        // The command a passing check ran still says what must not break.
        assert!(fix.contains("cargo test") && !fix.contains("passing-suite-noise"));
        // Both ends of the failure survive; the middle does not.
        assert!(fix.contains("first-error") && fix.contains("last-error"));
        assert!(fix.contains("bytes cut") && !fix.contains(&noise));
    }

    #[test]
    fn clipping_cuts_on_character_boundaries() {
        let text = "é".repeat(200);
        let cut = clip(&text, 64);
        assert!(cut.len() < text.len() && cut.contains("bytes cut"));
        assert!(cut.starts_with('é') && cut.ends_with('é'));
        assert_eq!(clip("short", 64), "short");
    }

    #[test]
    fn built_bundles_are_never_listed_as_the_files_a_prompt_is_about() {
        let signals = RepoSignals {
            tracked_paths: vec![
                "public/build/assets/QuoteView-Crz1qG.js".into(),
                "public/workbox-a9a38afb.js".into(),
                "app/Http/Controllers/QuoteController.php".into(),
            ],
            ..Default::default()
        };
        let r = route("fix the quote controller", Mode::Balanced, &signals);
        assert_eq!(r.candidate_paths, ["app/Http/Controllers/QuoteController.php"]);
        assert!(generated("public/build/assets/x.js") && !generated("public/index.php"));
    }

    #[test]
    fn earlier_work_is_carried_only_where_it_touched_the_files_the_prompt_is_about() {
        let past = |id, files: &[&str]| PastNote {
            id,
            note: format!("Task: earlier {id}"),
            files: files.iter().map(|f| f.to_string()).collect(),
        };
        let signals = RepoSignals {
            tracked_paths: vec![
                "app/Http/Controllers/QuoteController.php".into(),
                "app/Models/Invoice.php".into(),
            ],
            past: vec![
                past(2, &["app/Models/Invoice.php"]),
                past(1, &["app/Http/Controllers/QuoteController.php"]),
            ],
            ..Default::default()
        };
        let r = route("fix the quote controller", Mode::Balanced, &signals);
        assert_eq!(r.prior_notes, ["Task: earlier 1"]);
        assert!(r.prior_notes_why[0].contains("task #1") && r.prior_notes_why[0].contains("QuoteController"));
        let text = brief(&r, Stage::Implement, "fix the quote controller", &[], &[]);
        assert!(text.contains("Task: earlier 1") && !text.contains("earlier 2"));
        assert!(!brief(&r, Stage::Verify, "fix the quote controller", &[], &[]).contains("earlier 1"));

        let r = route("rename the login page label", Mode::Balanced, &signals);
        assert!(r.prior_notes.is_empty(), "no shared file, no history");
    }

    /// The brief names paths so the agent stops hunting. It must never carry
    /// the bytes themselves - that pays for the same context twice.
    #[test]
    fn a_brief_names_paths_and_pastes_no_contents() {
        let r = balanced("fix the run.rs typo", REPO);
        let text = brief(&r, Stage::Implement, "fix the run.rs typo", &[], &[]);
        assert!(
            text.contains("src/run.rs"),
            "the candidate path was not named: {text}"
        );
        assert!(
            !text.contains("fn main"),
            "file contents leaked into the brief"
        );
        assert!(r.candidate_paths.len() <= 10);
    }

    /// M5's rule, carried into every stage: an instruction never expires.
    #[test]
    fn every_stage_brief_repeats_every_instruction_the_user_has_given() {
        let r = balanced("redesign the storage subsystem", REPO);
        let constraints = vec![
            "keep the public API".to_string(),
            "no new dependencies".to_string(),
        ];
        for stage in &r.stages {
            let text = brief(
                &r,
                *stage,
                "redesign the storage subsystem",
                &constraints,
                &[],
            );
            for c in &constraints {
                assert!(text.contains(c.as_str()), "{} lost `{c}`", stage.name());
            }
        }
    }

    #[test]
    fn a_stage_with_no_artifact_is_forwarded_as_words_and_never_as_a_plan() {
        let r = balanced("redesign the storage subsystem", REPO);
        let note = StageNote {
            stage: Stage::Plan,
            model: None,
            effort: None,
            duration_ms: None,
            summary: "I think we should start with the store".into(),
            artifact: None,
        };
        let text = brief(
            &r,
            Stage::Implement,
            "redesign the storage subsystem",
            &[],
            &[note],
        );
        assert!(
            text.contains("no structured artifact"),
            "prose was passed off as an artifact"
        );
        assert!(text.contains("unvalidated"));
    }

    #[test]
    fn handoffs_only_carry_information_the_next_stage_cannot_recover() {
        let r = balanced("redesign the storage subsystem", REPO);
        let plan = StageNote {
            stage: Stage::Plan,
            model: None,
            effort: None,
            duration_ms: None,
            summary: "plan summary".into(),
            artifact: Some(serde_json::json!({
                "objective": "private-plan-marker", "constraints": [], "affected_areas": [],
                "implementation_steps": ["one"], "risks": [], "tests_required": []
            })),
        };
        let implementation = StageNote {
            stage: Stage::Implement,
            model: None,
            effort: None,
            duration_ms: None,
            summary: "implementation-marker".into(),
            artifact: None,
        };
        let failed = StageNote {
            stage: Stage::Verify,
            model: None,
            effort: None,
            duration_ms: None,
            summary: "failed check".into(),
            artifact: Some(serde_json::json!({
                "checks": [{"command": "npm test", "passed": false, "output": "boom"}],
                "verdict": "fail"
            })),
        };

        let implement = brief(
            &r,
            Stage::Implement,
            "task",
            &[],
            std::slice::from_ref(&plan),
        );
        assert!(implement.contains("private-plan-marker"));
        let fix = brief(
            &r,
            Stage::Fix,
            "task",
            &[],
            &[plan.clone(), implementation.clone(), failed.clone()],
        );
        assert!(
            fix.contains("boom")
                && !fix.contains("private-plan-marker")
                && !fix.contains("implementation-marker")
        );
        let review = brief(
            &r,
            Stage::Review,
            "task",
            &[],
            &[plan, implementation, failed],
        );
        assert!(
            !review.contains("private-plan-marker")
                && !review.contains("implementation-marker")
                && !review.contains("boom")
        );
    }

    #[test]
    fn only_a_well_formed_artifact_counts_as_one() {
        let good = serde_json::json!({
            "objective": "x", "constraints": [], "affected_areas": [],
            "implementation_steps": ["a"], "risks": [], "tests_required": []
        });
        assert!(artifact_is_valid(Stage::Plan, &good));
        // A plan missing a required list is a half-built plan, not a plan.
        let mut partial = good.clone();
        partial.as_object_mut().unwrap().remove("risks");
        assert!(!artifact_is_valid(Stage::Plan, &partial));
        assert!(!artifact_is_valid(
            Stage::Plan,
            &serde_json::json!("a plan, honest")
        ));
        // A Plan artifact is not a Review artifact, however well-formed.
        assert!(!artifact_is_valid(Stage::Review, &good));

        let review = |v: serde_json::Value| stage_passed(Stage::Review, &v);
        assert!(review(
            serde_json::json!({"findings": [], "verdict": "pass"})
        ));
        assert!(!review(
            serde_json::json!({"findings": [], "verdict": "changes_requested"})
        ));
        let finding = |severity: &str| {
            serde_json::json!({"severity": severity, "file": "a.rs", "line": 1, "issue": "i", "fix": "f"})
        };
        assert!(
            review(serde_json::json!({"findings": [finding("low")], "verdict": "changes_requested"})),
            "low findings are notes, not another round"
        );
        assert!(!review(
            serde_json::json!({"findings": [finding("low"), finding("medium")], "verdict": "changes_requested"})
        ));
        // Not an artifact at all, so not a pass: a missing review is not a
        // clean one.
        assert!(!review(serde_json::json!({"verdict": "pass"})));
    }

    /// A Verify pass is only a pass with evidence behind it: at least one check,
    /// and none of them failed, whatever the verdict says.
    #[test]
    fn a_verify_pass_needs_a_check_and_no_failing_one() {
        let verify = |v: serde_json::Value| stage_passed(Stage::Verify, &v);
        let ok =
            serde_json::json!({"command": "cargo test slug", "passed": true, "output": "1 passed"});
        let bad =
            serde_json::json!({"command": "cargo test", "passed": false, "output": "1 failed"});
        assert!(verify(
            serde_json::json!({"checks": [ok], "verdict": "pass"})
        ));
        assert!(
            !verify(serde_json::json!({"checks": [], "verdict": "pass"})),
            "a pass with nothing run"
        );
        assert!(
            !verify(serde_json::json!({"checks": [ok, bad], "verdict": "pass"})),
            "a pass over a failing check"
        );
        assert!(!verify(
            serde_json::json!({"checks": [ok], "verdict": "fail"})
        ));
        assert!(!verify(serde_json::json!({"verdict": "pass"})));
    }

    /// Scope that was never shown to be small does not get the one-call route,
    /// and a cause nobody has found yet is not trivial because the words are few.
    #[test]
    fn unknown_scope_and_unfound_causes_leave_the_one_call_route() {
        assert_eq!(
            balanced("make the header bold", REPO).kind,
            RouteKind::Standard
        );
        assert_eq!(
            balanced("fix the typo", REPO).kind,
            RouteKind::ImplementOnce
        );
        assert_eq!(balanced("tidy run.rs", REPO).kind, RouteKind::ImplementOnce);
        assert_ne!(
            balanced("fix the intermittent hang in run.rs", REPO).kind,
            RouteKind::ImplementOnce
        );
        assert_ne!(
            balanced("the watcher is flaky", REPO).kind,
            RouteKind::ImplementOnce
        );
        // " hang" is not a substring of "change".
        assert_eq!(
            balanced("change the typo in run.rs", REPO).kind,
            RouteKind::ImplementOnce
        );
    }

    #[test]
    fn only_implement_goes_without_an_artifact_and_only_implement_writes() {
        assert!(Stage::Plan.schema().is_some());
        assert!(Stage::Review.schema().is_some());
        assert!(Stage::Verify.schema().is_some());
        assert_eq!(Stage::Fix.schema(), Some(VERIFY_SCHEMA));
        assert!(Stage::Implement.schema().is_none());
        assert!(Stage::Implement.writes());
        assert!(Stage::Fix.writes());
        for stage in [Stage::Plan, Stage::Review, Stage::Verify] {
            assert!(
                !stage.writes(),
                "{} must not be allowed to edit",
                stage.name()
            );
        }
        // Every schema has to be JSON a CLI will accept.
        for schema in [PLAN_SCHEMA, REVIEW_SCHEMA, VERIFY_SCHEMA] {
            serde_json::from_str::<serde_json::Value>(schema).expect("schema must be valid JSON");
        }
    }

    #[test]
    fn the_capability_map_is_recorded_and_not_acted_on() {
        let r = balanced("redesign the storage subsystem", REPO);
        assert_eq!(r.preferred_providers.len(), r.stages.len());
        assert_eq!(Capability::Deep.preferred_provider(), ProviderId::Claude);
        assert_eq!(Capability::Review.preferred_provider(), ProviderId::Claude);
        assert_eq!(
            Capability::Implement.preferred_provider(),
            ProviderId::Codex
        );
    }

    #[test]
    fn an_empty_prompt_names_no_candidate_paths() {
        let r = balanced("", REPO);
        assert!(r.candidate_paths.is_empty());
        assert_eq!(r.signals.blast_radius, 0);
    }
}
