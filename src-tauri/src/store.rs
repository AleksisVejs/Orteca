//! SQLite. Migrations are numbered SQL files applied in order, tracked with
//! `PRAGMA user_version`. No ORM, no query builder.

use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::error::{AppError, ErrorKind, Result};
use crate::project::FileStat;
use crate::providers::{CostQuality, Usage};
use crate::routing::{RouteKind, Tier};

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_tasks.sql"),
    include_str!("../migrations/0003_calls.sql"),
    include_str!("../migrations/0004_task_details.sql"),
    include_str!("../migrations/0005_worktree.sql"),
    include_str!("../migrations/0006_model_prices.sql"),
];

/// A published API rate, in USD per million tokens.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Price {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

/// What comparable finished runs in a project have cost. Always an estimate:
/// "comparable" means the same route kind on the same provider, which is not
/// the same work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Baseline {
    pub runs: u32,
    pub median_tokens: u64,
    pub median_calls: u32,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub id: i64,
    pub path: String,
    pub name: String,
    pub trusted: bool,
    pub last_opened_at: String,
}

/// Everything a task row needs at the moment it opens. A struct rather than
/// eight positional arguments, which is how `mode` and `route_json` would end
/// up swapped one day.
pub struct NewTask<'a> {
    pub project_id: i64,
    pub prompt: &'a str,
    pub mode: &'a str,
    /// The route, serialised. Decided before any provider starts.
    pub route_json: Option<&'a str>,
    pub branch: Option<&'a str>,
    pub base_commit: Option<&'a str>,
    pub dirty_at_start: bool,
}

pub struct Store(Mutex<Connection>, #[allow(dead_code)] Option<std::fs::File>);

impl Store {
    #[cfg(test)]
    pub fn reject_events(&self) {
        self.0.lock().unwrap().execute_batch("CREATE TRIGGER reject_event BEFORE INSERT ON task_events BEGIN SELECT RAISE(ABORT, 'disk unavailable'); END;").unwrap();
    }

    #[cfg(test)]
    pub fn event_kinds(&self, task: i64) -> Vec<String> {
        let conn = self.0.lock().unwrap();
        let mut statement = conn
            .prepare("SELECT kind FROM task_events WHERE task_id=?1 ORDER BY id")
            .unwrap();
        statement
            .query_map([task], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|k| k.unwrap())
            .collect()
    }

    #[cfg(test)]
    pub fn event_payloads(&self, task: i64) -> Vec<serde_json::Value> {
        let conn = self.0.lock().unwrap();
        let mut statement = conn
            .prepare("SELECT payload_json FROM task_events WHERE task_id=?1 ORDER BY id")
            .unwrap();
        statement
            .query_map([task], |r| r.get::<_, String>(0))
            .unwrap()
            .map(|s| serde_json::from_str(&s.unwrap()).unwrap())
            .collect()
    }

    pub fn open(db_path: &Path) -> Result<Self> {
        if let Some(dir) = db_path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        // Startup recovery must never sweep another instance's live tasks.
        let owner = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .share_mode(0)
            .open(db_path.with_extension("owner"))?;
        let conn = Connection::open(db_path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        migrate(&conn)?;
        conn.execute("UPDATE tasks SET status = 'failed', ended_at = datetime('now'), summary = 'Interrupted when Orteca closed' WHERE status = 'running'", [])?;
        Ok(Store(Mutex::new(conn), Some(owner)))
    }

    #[cfg(test)]
    pub fn in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        migrate(&conn)?;
        Ok(Store(Mutex::new(conn), None))
    }

    /// Insert or refresh a project and stamp it as just opened.
    pub fn touch_project(&self, path: &str, name: &str) -> Result<Project> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute(
            "INSERT INTO projects (path, name, last_opened_at, opened_seq)
             VALUES (?1, ?2, datetime('now'),
                     (SELECT IFNULL(MAX(opened_seq), 0) + 1 FROM projects))
             ON CONFLICT(path) DO UPDATE SET
               name = excluded.name,
               last_opened_at = excluded.last_opened_at,
               opened_seq = (SELECT MAX(opened_seq) + 1 FROM projects)",
            params![path, name],
        )?;
        read_project(&conn, path)
    }

    pub fn project(&self, path: &str) -> Result<Project> {
        let conn = self.0.lock().expect("store poisoned");
        read_project(&conn, path)
    }

    /// Open a task in `running`. The baseline is written now, before the agent
    /// touches anything, so the diff at the end has something honest to compare
    /// against - and so is the route, which is decided before any provider is
    /// started and must be readable afterwards whatever became of the run.
    pub fn create_task(&self, task: NewTask) -> Result<i64> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute(
            "INSERT INTO tasks
                (project_id, prompt, mode, route_json, status, branch,
                 base_commit, dirty_at_start, started_at)
             VALUES (?1, ?2, ?3, ?4, 'running', ?5, ?6, ?7, datetime('now'))",
            params![
                task.project_id,
                task.prompt,
                task.mode,
                task.route_json,
                task.branch,
                task.base_commit,
                task.dirty_at_start
            ],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// A reply continues its task: the same row runs again on a new route. A
    /// running task, a task in a copy folder or another project's is refused.
    pub fn reopen_task(&self, project_id: i64, task_id: i64, route_json: Option<&str>) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        let changed = conn.execute(
            "UPDATE tasks SET status = 'running', route_json = ?3, ended_at = NULL
              WHERE id = ?1 AND project_id = ?2 AND status != 'running' AND worktree_path IS NULL",
            params![task_id, project_id, route_json],
        )?;
        if changed == 0 {
            return Err(AppError::new(ErrorKind::Invalid, "That task cannot be continued."));
        }
        Ok(())
    }

    /// The copy a run works in, recorded the moment it exists, so a run that
    /// fails straight after can still have it removed.
    pub fn set_worktree(&self, task_id: i64, branch: &str, path: &str) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute(
            "UPDATE tasks SET branch = ?2, worktree_path = ?3 WHERE id = ?1",
            params![task_id, branch, path],
        )?;
        Ok(())
    }

    /// A finished task's copy. A running task's is never offered for removal:
    /// the agent is still working in it.
    pub fn worktree_path(&self, project_id: i64, task_id: i64) -> Result<Option<String>> {
        let conn = self.0.lock().expect("store poisoned");
        match conn.query_row(
            "SELECT worktree_path FROM tasks WHERE project_id = ?1 AND id = ?2 AND status != 'running'",
            params![project_id, task_id],
            |r| r.get(0),
        ) {
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            other => Ok(other?),
        }
    }

    pub fn clear_worktree(&self, project_id: i64, task_id: i64) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute(
            "UPDATE tasks SET worktree_path = NULL WHERE project_id = ?1 AND id = ?2",
            params![project_id, task_id],
        )?;
        Ok(())
    }

    /// How many earlier runs of this exact prompt in this project ended without
    /// finishing. Two is what the architecture escalates on.
    ///
    /// Exact prompt text, deliberately: "the same task" has no other honest
    /// definition available without a model call, and a fuzzy match that
    /// escalated the wrong task would spend the user's money on a route they
    /// did not need. A re-worded retry counts as a fresh task, which errs
    /// towards the cheaper route.
    ///
    /// `budgetReached`, `reviewRejected` and `verifyFailed` count. A run that ran
    /// out of budget, was rejected by review or failed its checks did not finish
    /// its work, and pretending otherwise would leave a task looping on a route
    /// that could not finish.
    pub fn prior_failures(&self, project_id: i64, prompt: &str) -> Result<u32> {
        let conn = self.0.lock().expect("store poisoned");
        Ok(conn.query_row(
            // A spent plan, a rate limit or an expired sign-in says nothing
            // about whether the prompt is hard. Counting one would send the
            // same prompt, continued on the other CLI, a tier up.
            "SELECT COUNT(*) FROM tasks t
              WHERE t.project_id = ?1 AND t.prompt = ?2
                AND t.status IN ('failed', 'budgetReached', 'reviewRejected', 'verifyFailed')
                AND NOT EXISTS (SELECT 1 FROM task_events e
                                 WHERE e.task_id = t.id AND e.kind = 'failed'
                                   AND json_extract(e.payload_json, '$.data.kind') IN ('usageLimit', 'rateLimit', 'authExpired'))",
            params![project_id, prompt],
            |r| r.get(0),
        )?)
    }

    pub fn append_event(
        &self,
        task_id: i64,
        stage: &str,
        kind: &str,
        provider: &str,
        payload_json: &str,
    ) -> Result<i64> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute(
            "INSERT INTO task_events (task_id, ts, stage, kind, provider, payload_json)
             VALUES (?1, datetime('now'), ?2, ?3, ?4, ?5)",
            params![task_id, stage, kind, provider, payload_json],
        )?;
        Ok(conn.last_insert_rowid())
    }

    /// Replace the whole price list. An empty fetch keeps the last one, so a
    /// bad download never leaves every model unpriced.
    pub fn save_prices(&self, prices: &[(String, Price)]) -> Result<()> {
        if prices.is_empty() {
            return Ok(());
        }
        let conn = self.0.lock().expect("store poisoned");
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM model_prices", [])?;
        for (model, p) in prices {
            tx.execute(
                "INSERT INTO model_prices (model, input, output, cache_read, fetched_at)
                 VALUES (?1, ?2, ?3, ?4, datetime('now'))",
                params![model, p.input, p.output, p.cache_read],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn price(&self, model: &str) -> Option<Price> {
        let conn = self.0.lock().expect("store poisoned");
        conn.query_row(
            "SELECT input, output, cache_read FROM model_prices WHERE model = ?1",
            [model],
            |r| {
                Ok(Price {
                    input: r.get(0)?,
                    output: r.get(1)?,
                    cache_read: r.get(2)?,
                })
            },
        )
        .ok()
    }

    /// `None` means the run ended before the provider reported anything. That
    /// is recorded as `unavailable` with every count left NULL - never as a
    /// zero, which would read as "this run was free".
    pub fn record_usage(
        &self,
        task_id: i64,
        event_id: Option<i64>,
        provider: &str,
        usage: Option<&Usage>,
    ) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        let quality = usage.map_or(CostQuality::Unavailable, |u| u.cost_quality);
        let tx = conn.unchecked_transaction()?;
        tx.execute("DELETE FROM usage WHERE task_id = ?1", [task_id])?;
        tx.execute(
            "INSERT INTO usage
                (task_id, event_id, provider, input_tokens, cached_input_tokens,
                 output_tokens, reasoning_tokens, cost_usd, cost_quality, model)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                task_id,
                event_id,
                provider,
                usage.map(|u| u.input_tokens),
                usage.map(|u| u.cached_input_tokens),
                usage.map(|u| u.output_tokens),
                usage.map(|u| u.reasoning_tokens),
                usage.and_then(|u| u.cost_usd),
                quality_name(quality),
                usage.and_then(|u| u.model.as_deref()),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// A continued task's usage: this turn added to what the task already
    /// spent. Each turn is its own process, so costs add too. A turn with no
    /// numbers leaves the total unavailable, never smaller than it was.
    pub fn add_usage(
        &self,
        task_id: i64,
        provider: &str,
        usage: Option<&Usage>,
    ) -> Result<()> {
        type Row = (Option<i64>, Option<i64>, Option<i64>, Option<i64>, Option<f64>, String);
        let before: Option<Row> = {
            let conn = self.0.lock().expect("store poisoned");
            match conn.query_row(
                "SELECT input_tokens, cached_input_tokens, output_tokens, reasoning_tokens,
                        cost_usd, cost_quality
                   FROM usage WHERE task_id = ?1",
                [task_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
            ) {
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                other => Some(other?),
            }
        };
        let total = match (before, usage) {
            (None, usage) => usage.cloned(),
            (Some((Some(i), Some(c), Some(o), Some(r), cost, quality)), Some(u)) => {
                let cost_usd = cost.zip(u.cost_usd).map(|(a, b)| a + b);
                Some(Usage {
                    model: u.model.clone(),
                    input_tokens: i as u64 + u.input_tokens,
                    cached_input_tokens: c as u64 + u.cached_input_tokens,
                    output_tokens: o as u64 + u.output_tokens,
                    reasoning_tokens: r as u64 + u.reasoning_tokens,
                    cost_usd,
                    cost_quality: match (cost_usd, quality.as_str(), u.cost_quality) {
                        (None, ..) => CostQuality::Unavailable,
                        (_, "exact", CostQuality::Exact) => CostQuality::Exact,
                        _ => CostQuality::Estimated,
                    },
                })
            }
            _ => None,
        };
        self.record_usage(task_id, None, provider, total.as_ref())
    }

    #[cfg(test)]
    pub fn finish_task(
        &self,
        task_id: i64,
        status: &str,
        summary: &str,
        diff_stat_json: &str,
        calls_used: u32,
    ) -> Result<()> {
        self.finish_task_details(
            task_id,
            status,
            summary,
            diff_stat_json,
            None,
            0,
            None,
            calls_used,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn finish_task_details(
        &self,
        task_id: i64,
        status: &str,
        summary: &str,
        diff_stat_json: &str,
        patch_text: Option<&str>,
        unknown_events: u32,
        duration_ms: Option<u64>,
        calls_used: u32,
    ) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        let changed = conn.execute(
            "UPDATE tasks
                SET status = ?2, summary = ?3, diff_stat_json = ?4,
                    calls_used = COALESCE(calls_used, 0) + ?5,
                    patch_text = ?6, unknown_events = unknown_events + ?7,
                    duration_ms = CASE WHEN ?8 IS NULL THEN duration_ms
                                       ELSE COALESCE(duration_ms, 0) + ?8 END,
                    ended_at = datetime('now')
              WHERE id = ?1",
            params![
                task_id,
                status,
                summary,
                diff_stat_json,
                calls_used,
                patch_text,
                unknown_events,
                duration_ms.map(|ms| ms.min(i64::MAX as u64) as i64),
            ],
        )?;
        if changed == 0 {
            return Err(AppError::new(
                ErrorKind::NotFound,
                "task record no longer exists",
            ));
        }
        Ok(())
    }

    /// The median of the last 20 runs in this task's project that finished
    /// `done` on the same route kind, provider and mode, and reported usage.
    /// Tokens exclude cache reads, so a warm cache does not read as less work. `None`
    /// below five of them: a median of two runs is an anecdote, and the screen
    /// shows absolute numbers until there is more.
    pub fn baseline(
        &self,
        task_id: i64,
        route_kind: &str,
        provider: &str,
    ) -> Result<Option<Baseline>> {
        const MIN_RUNS: usize = 5;
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT u.input_tokens + u.output_tokens, t.calls_used
               FROM tasks t JOIN usage u ON u.task_id = t.id
              WHERE (t.project_id, t.mode) = (SELECT project_id, mode FROM tasks WHERE id = ?1)
                AND t.id != ?1 AND t.status = 'done' AND t.calls_used IS NOT NULL
                AND u.input_tokens IS NOT NULL AND u.provider = ?3
                AND json_extract(t.route_json, '$.kind') = ?2
              ORDER BY t.id DESC LIMIT 20",
        )?;
        let rows = stmt
            .query_map(params![task_id, route_kind, provider], |r| {
                Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        if rows.len() < MIN_RUNS {
            return Ok(None);
        }
        let (mut tokens, mut calls): (Vec<i64>, Vec<i64>) = rows.into_iter().unzip();
        Ok(Some(Baseline {
            runs: tokens.len() as u32,
            median_tokens: median(&mut tokens) as u64,
            median_calls: median(&mut calls) as u32,
        }))
    }

    /// Route kinds and tiers that stalled in this project on this provider and
    /// mode: at least five finished runs among the project's last 50, two in
    /// five of them `budgetReached`, `reviewRejected` or `verifyFailed`.
    /// `failed` is left out, because a rate limit or an expired sign-in says
    /// nothing about the model. Mode is kept apart because the two modes run
    /// different tiers. A run that needed fixes and then finished is not a
    /// stall: fixing until the checks pass is how a route works.
    // ponytail: a Codex implement that changed nothing is `failed` and so not
    // counted; split failure kinds into their own column if that hides stalls.
    pub fn stalled_tiers(
        &self,
        project_id: i64,
        provider: &str,
        mode: &str,
    ) -> Result<Vec<(RouteKind, Tier)>> {
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT json_extract(t.route_json, '$.kind'), json_extract(t.route_json, '$.budget.preferredTier')
               FROM tasks t JOIN usage u ON u.task_id = t.id
              WHERE t.id IN (SELECT id FROM tasks WHERE project_id = ?1 ORDER BY id DESC LIMIT 50)
                AND u.provider = ?2
                AND t.mode = ?3
                AND t.status IN ('done', 'budgetReached', 'reviewRejected', 'verifyFailed')
              GROUP BY 1, 2
             HAVING COUNT(*) >= 5
                AND 5 * SUM(t.status != 'done') >= 2 * COUNT(*)",
        )?;
        let rows = stmt
            .query_map(params![project_id, provider, mode], |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?,
                    r.get::<_, Option<String>>(1)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        // A row from before tiers were recorded names none, and is skipped.
        Ok(rows
            .into_iter()
            .filter_map(|(kind, tier)| {
                let parse = |s: Option<String>| s.map(serde_json::Value::String);
                Some((
                    serde_json::from_value(parse(kind)?).ok()?,
                    serde_json::from_value(parse(tier)?).ok()?,
                ))
            })
            .collect())
    }

    pub fn set_trusted(&self, path: &str, trusted: bool) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute(
            "UPDATE projects
                SET trusted = ?2, trust_scanned_at = datetime('now')
              WHERE path = ?1",
            params![path, trusted],
        )?;
        Ok(())
    }

    pub fn recent_projects(&self, limit: u32) -> Result<Vec<Project>> {
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, path, name, trusted, last_opened_at
               FROM projects ORDER BY opened_seq DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], row_to_project)?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn forget_project(&self, path: &str) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        conn.execute("DELETE FROM projects WHERE path = ?1", params![path])?;
        Ok(())
    }

    /// A project's runs, newest first. Token counts stay NULL where the
    /// provider reported none, as they are in `usage`.
    pub fn recent_tasks(&self, project_id: i64, limit: u32) -> Result<Vec<TaskSummary>> {
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT t.id, t.prompt, t.status, t.started_at, t.summary,
                    json_extract(t.route_json, '$.kind'), t.calls_used, u.provider, u.model,
                    u.input_tokens + u.cached_input_tokens + u.output_tokens,
                    u.input_tokens + u.output_tokens, u.cached_input_tokens, u.cost_usd,
                    u.cost_quality, t.unknown_events, t.duration_ms,
                    (t.patch_text IS NOT NULL AND length(t.patch_text) > 0)
               FROM tasks t LEFT JOIN usage u ON u.task_id = t.id
              WHERE t.project_id = ?1
              ORDER BY t.id DESC LIMIT ?2",
        )?;
        let rows = stmt.query_map(params![project_id, limit], |r| {
            Ok(TaskSummary {
                id: r.get(0)?,
                prompt: r.get(1)?,
                status: r.get(2)?,
                started_at: r.get(3)?,
                summary: r.get(4)?,
                route_kind: r.get(5)?,
                calls_used: r.get(6)?,
                provider: r.get(7)?,
                model: r.get(8)?,
                tokens: r.get(9)?,
                uncached_tokens: r.get(10)?,
                cached_tokens: r.get(11)?,
                cost_usd: r.get(12)?,
                cost_quality: r.get(13)?,
                unknown_events: r.get(14)?,
                duration_ms: r.get(15)?,
                patch_available: r.get(16)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn task_detail(&self, project_id: i64, task_id: i64) -> Result<TaskDetail> {
        let conn = self.0.lock().expect("store poisoned");
        let mut detail = conn
            .query_row(
                "SELECT t.id, t.prompt, t.status, t.started_at, t.ended_at, t.summary,
                    t.route_json, t.diff_stat_json, t.patch_text, t.dirty_at_start,
                    t.calls_used, u.provider, u.model,
                    u.input_tokens + u.cached_input_tokens + u.output_tokens,
                    u.input_tokens + u.output_tokens, u.cached_input_tokens,
                    u.cost_usd, u.cost_quality, t.unknown_events, t.duration_ms,
                    t.branch, t.worktree_path
               FROM tasks t LEFT JOIN usage u ON u.task_id = t.id
              WHERE t.project_id = ?1 AND t.id = ?2",
                params![project_id, task_id],
                |r| {
                    let route_json: Option<String> = r.get(6)?;
                    let diff_json: Option<String> = r.get(7)?;
                    Ok(TaskDetail {
                        id: r.get(0)?,
                        prompt: r.get(1)?,
                        status: r.get(2)?,
                        started_at: r.get(3)?,
                        ended_at: r.get(4)?,
                        summary: r.get(5)?,
                        route: route_json.and_then(|json| serde_json::from_str(&json).ok()),
                        diff: diff_json
                            .and_then(|json| serde_json::from_str(&json).ok())
                            .unwrap_or_default(),
                        patch_text: r.get(8)?,
                        dirty_at_start: r.get::<_, i64>(9)? != 0,
                        calls_used: r.get(10)?,
                        provider: r.get(11)?,
                        model: r.get(12)?,
                        tokens: r.get(13)?,
                        uncached_tokens: r.get(14)?,
                        cached_tokens: r.get(15)?,
                        cost_usd: r.get(16)?,
                        cost_quality: r.get(17)?,
                        unknown_events: r.get(18)?,
                        duration_ms: r.get(19)?,
                        branch: r.get(20)?,
                        worktree_path: r.get(21)?,
                        events: Vec::new(),
                    })
                },
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    AppError::new(ErrorKind::NotFound, "that task is not in this project")
                }
                other => other.into(),
            })?;

        let mut events = conn.prepare(
            "SELECT id, ts, stage, kind, provider, payload_json
               FROM task_events WHERE task_id = ?1 ORDER BY id",
        )?;
        detail.events = events
            .query_map([task_id], |r| {
                let payload: String = r.get(5)?;
                Ok(TaskEvent {
                    id: r.get(0)?,
                    ts: r.get(1)?,
                    stage: r.get(2)?,
                    kind: r.get(3)?,
                    provider: r.get(4)?,
                    payload: serde_json::from_str(&payload)
                        .unwrap_or(serde_json::Value::String(payload)),
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(detail)
    }
}

/// One row of a project's run history.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub id: i64,
    pub prompt: String,
    pub status: String,
    /// SQLite `datetime('now')`: UTC, to the second.
    pub started_at: String,
    pub summary: Option<String>,
    pub route_kind: Option<String>,
    pub calls_used: Option<u32>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tokens: Option<u64>,
    pub uncached_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub cost_quality: Option<String>,
    pub unknown_events: u32,
    pub duration_ms: Option<u64>,
    pub patch_available: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskEvent {
    pub id: i64,
    pub ts: String,
    pub stage: Option<String>,
    pub kind: String,
    pub provider: String,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskDetail {
    pub id: i64,
    pub prompt: String,
    pub status: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    pub summary: Option<String>,
    pub route: Option<serde_json::Value>,
    pub diff: Vec<FileStat>,
    pub patch_text: Option<String>,
    pub dirty_at_start: bool,
    pub calls_used: Option<u32>,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub tokens: Option<u64>,
    pub uncached_tokens: Option<u64>,
    pub cached_tokens: Option<u64>,
    pub cost_usd: Option<f64>,
    pub cost_quality: Option<String>,
    pub unknown_events: u32,
    pub duration_ms: Option<u64>,
    pub branch: Option<String>,
    /// The separate copy the run worked in, until it is removed.
    pub worktree_path: Option<String>,
    pub events: Vec<TaskEvent>,
}

fn read_project(conn: &Connection, path: &str) -> Result<Project> {
    conn.query_row(
        "SELECT id, path, name, trusted, last_opened_at FROM projects WHERE path = ?1",
        params![path],
        row_to_project,
    )
    .map_err(|e| match e {
        rusqlite::Error::QueryReturnedNoRows => {
            AppError::new(ErrorKind::NotFound, "that project is not open any more")
        }
        other => other.into(),
    })
}

/// The upper middle of an even count. Good enough for a figure labelled
/// estimated; not a statistic anyone should quote.
fn median(values: &mut [i64]) -> i64 {
    values.sort_unstable();
    values[values.len() / 2]
}

fn quality_name(q: CostQuality) -> &'static str {
    match q {
        CostQuality::Exact => "exact",
        CostQuality::Estimated => "estimated",
        CostQuality::Unavailable => "unavailable",
    }
}

fn row_to_project(row: &rusqlite::Row) -> rusqlite::Result<Project> {
    Ok(Project {
        id: row.get(0)?,
        path: row.get(1)?,
        name: row.get(2)?,
        trusted: row.get::<_, i64>(3)? != 0,
        last_opened_at: row.get(4)?,
    })
}

fn migrate(conn: &Connection) -> Result<()> {
    let applied: u32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    for (i, sql) in MIGRATIONS.iter().enumerate().skip(applied as usize) {
        let tx = conn.unchecked_transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", (i + 1) as i64)?;
        tx.commit()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn new_task<'a>(project_id: i64, prompt: &'a str, mode: &'a str) -> NewTask<'a> {
        NewTask {
            project_id,
            prompt,
            mode,
            route_json: None,
            branch: None,
            base_commit: None,
            dirty_at_start: false,
        }
    }

    #[test]
    fn a_second_store_cannot_reconcile_a_live_instance() {
        let dir = std::env::temp_dir().join(format!("orteca-owner-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        let first = Store::open(&db).unwrap();
        assert!(Store::open(&db).is_err());
        drop(first);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn usage_is_one_snapshot_per_task() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("a", "a").unwrap();
        let task = store
            .create_task(new_task(project.id, "test", "balanced"))
            .unwrap();
        store.record_usage(task, None, "codex", None).unwrap();
        store.record_usage(task, None, "codex", None).unwrap();
        let count: i64 = store
            .0
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM usage", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
    }

    #[test]
    fn history_is_newest_first_and_never_a_zero_for_unknown_usage() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("a", "a").unwrap();
        let silent = store
            .create_task(new_task(project.id, "first", "balanced"))
            .unwrap();
        store.record_usage(silent, None, "codex", None).unwrap();
        let measured = store
            .create_task(new_task(project.id, "second", "balanced"))
            .unwrap();
        let usage = Usage {
            model: None,
            input_tokens: 10,
            cached_input_tokens: 100,
            output_tokens: 5,
            reasoning_tokens: 0,
            cost_usd: Some(0.01),
            cost_quality: CostQuality::Estimated,
        };
        store
            .record_usage(measured, None, "claude", Some(&usage))
            .unwrap();
        store.finish_task(measured, "done", "ok", "[]", 1).unwrap();

        let rows = store.recent_tasks(project.id, 20).unwrap();
        assert_eq!(
            rows.iter().map(|r| r.id).collect::<Vec<_>>(),
            [measured, silent]
        );
        assert_eq!(
            (
                rows[0].tokens,
                rows[0].calls_used,
                rows[0].cost_quality.as_deref()
            ),
            (Some(115), Some(1), Some("estimated"))
        );
        assert_eq!((rows[1].tokens, rows[1].status.as_str()), (None, "running"));
    }

    #[test]
    fn a_continued_task_adds_each_turn_to_its_totals() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("a", "a").unwrap();
        let task = store
            .create_task(new_task(project.id, "first", "balanced"))
            .unwrap();
        let turn = |input, cost, quality| Usage {
            model: None,
            input_tokens: input,
            cached_input_tokens: 0,
            output_tokens: 1,
            reasoning_tokens: 0,
            cost_usd: cost,
            cost_quality: quality,
        };
        store.add_usage(task, "claude", Some(&turn(10, Some(0.01), CostQuality::Exact))).unwrap();
        store.finish_task_details(task, "verifyFailed", "one", "[]", None, 2, Some(1000), 3).unwrap();
        assert!(
            store.reopen_task(project.id + 1, task, None).is_err(),
            "another project's task"
        );
        store.reopen_task(project.id, task, None).unwrap();
        assert!(store.reopen_task(project.id, task, None).is_err(), "already running");
        store.add_usage(task, "claude", Some(&turn(20, Some(0.02), CostQuality::Estimated))).unwrap();
        store.finish_task_details(task, "done", "two", "[]", None, 1, Some(500), 2).unwrap();

        let row = &store.recent_tasks(project.id, 20).unwrap()[0];
        assert_eq!(
            (row.prompt.as_str(), row.status.as_str(), row.summary.as_deref()),
            ("first", "done", Some("two"))
        );
        assert_eq!((row.tokens, row.calls_used, row.unknown_events, row.duration_ms), (Some(32), Some(5), 3, Some(1500)));
        assert_eq!(row.cost_quality.as_deref(), Some("estimated"));
        assert!((row.cost_usd.unwrap() - 0.03).abs() < 1e-9);

        // A turn with no numbers leaves the total unknown, not smaller.
        store.reopen_task(project.id, task, None).unwrap();
        store.add_usage(task, "claude", None).unwrap();
        let row = &store.recent_tasks(project.id, 20).unwrap()[0];
        assert_eq!((row.tokens, row.cost_quality.as_deref()), (None, Some("unavailable")));
    }

    #[test]
    fn task_detail_returns_patch_metrics_and_event_log_for_its_project() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("a", "a").unwrap();
        let route = r#"{"kind":"implementOnce"}"#;
        let task = store
            .create_task(NewTask {
                route_json: Some(route),
                ..new_task(project.id, "inspect it", "balanced")
            })
            .unwrap();
        let usage = Usage {
            model: Some("test-model".into()),
            input_tokens: 10,
            cached_input_tokens: 3,
            output_tokens: 5,
            reasoning_tokens: 0,
            cost_usd: None,
            cost_quality: CostQuality::Unavailable,
        };
        store
            .record_usage(task, None, "codex", Some(&usage))
            .unwrap();
        store
            .append_event(
                task,
                "implement",
                "unknown",
                "codex",
                r#"{"type":"new.event"}"#,
            )
            .unwrap();
        store
            .finish_task_details(
                task,
                "done",
                "finished",
                "[]",
                Some("diff --git"),
                1,
                Some(1250),
                1,
            )
            .unwrap();

        let detail = store.task_detail(project.id, task).unwrap();
        assert_eq!(detail.patch_text.as_deref(), Some("diff --git"));
        assert_eq!(detail.unknown_events, 1);
        assert_eq!(detail.duration_ms, Some(1250));
        assert_eq!(detail.model.as_deref(), Some("test-model"));
        assert_eq!(detail.events[0].kind, "unknown");
        assert!(store.task_detail(project.id + 1, task).is_err());
    }

    #[test]
    fn missing_task_cannot_be_finished_successfully() {
        assert!(Store::in_memory()
            .unwrap()
            .finish_task(99, "done", "", "[]", 1)
            .is_err());
    }

    #[test]
    fn startup_closes_interrupted_tasks() {
        let dir = std::env::temp_dir().join(format!("orteca-reconcile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let db = dir.join("test.db");
        let store = Store::open(&db).unwrap();
        let project = store.touch_project("a", "a").unwrap();
        let task = store
            .create_task(new_task(project.id, "test", "balanced"))
            .unwrap();
        drop(store);
        let store = Store::open(&db).unwrap();
        let (status, ended): (String, Option<String>) = store
            .0
            .lock()
            .unwrap()
            .query_row(
                "SELECT status, ended_at FROM tasks WHERE id=?1",
                [task],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "failed");
        assert!(ended.is_some());
        drop(store);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn migrations_are_idempotent() {
        let store = Store::in_memory().unwrap();
        let conn = store.0.lock().unwrap();
        let before: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(before, MIGRATIONS.len() as u32);
        // Re-running must be a no-op, not a "table already exists" error.
        migrate(&conn).unwrap();
    }

    #[test]
    fn failed_migration_rolls_back_schema() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE blocker (id); CREATE INDEX projects_recent ON blocker(id);",
        )
        .unwrap();
        assert!(migrate(&conn).is_err());
        let tables: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE name = 'projects'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(tables, 0);
        let version: u32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .unwrap();
        assert_eq!(version, 0);
    }

    #[test]
    fn repeated_opens_order_three_projects() {
        let store = Store::in_memory().unwrap();
        for name in ["a", "b", "c", "b", "a", "c", "a"] {
            store.touch_project(name, name).unwrap();
        }
        let paths: Vec<_> = store
            .recent_projects(10)
            .unwrap()
            .into_iter()
            .map(|p| p.path)
            .collect();
        assert_eq!(paths, ["a", "c", "b"]);
    }

    #[test]
    fn recents_are_newest_first_and_paths_are_unique() {
        let store = Store::in_memory().unwrap();
        store.touch_project("C:/a", "a").unwrap();
        store.touch_project("C:/b", "b").unwrap();
        // Same path twice must update, not duplicate.
        store.touch_project("C:/a", "a-renamed").unwrap();

        let recents = store.recent_projects(10).unwrap();
        assert_eq!(recents.len(), 2);
        assert_eq!(recents[0].path, "C:/a");
        assert_eq!(recents[0].name, "a-renamed");
    }

    #[test]
    fn a_run_with_no_reported_usage_is_recorded_as_unavailable() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("C:/a", "a").unwrap();
        let task = store
            .create_task(NewTask {
                branch: Some("main"),
                base_commit: Some("abc"),
                ..new_task(project.id, "do it", "balanced")
            })
            .unwrap();
        store.record_usage(task, None, "codex", None).unwrap();

        let conn = store.0.lock().unwrap();
        let (input, quality): (Option<i64>, String) = conn
            .query_row(
                "SELECT input_tokens, cost_quality FROM usage WHERE task_id = ?1",
                params![task],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        // A zero here would read as "this run was free".
        assert_eq!(input, None);
        assert_eq!(quality, "unavailable");
    }

    #[test]
    fn events_are_appended_in_order_and_the_task_closes() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("C:/a", "a").unwrap();
        let task = store
            .create_task(NewTask {
                dirty_at_start: true,
                ..new_task(project.id, "do it", "efficient")
            })
            .unwrap();
        for kind in ["started", "toolUse", "done"] {
            store
                .append_event(task, "run", kind, "codex", "{}")
                .unwrap();
        }
        store.finish_task(task, "done", "all set", "[]", 1).unwrap();

        let conn = store.0.lock().unwrap();
        let kinds: Vec<String> = conn
            .prepare("SELECT kind FROM task_events WHERE task_id = ?1 ORDER BY id")
            .unwrap()
            .query_map(params![task], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap();
        assert_eq!(kinds, ["started", "toolUse", "done"]);

        let (status, ended): (String, Option<String>) = conn
            .query_row(
                "SELECT status, ended_at FROM tasks WHERE id = ?1",
                params![task],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(status, "done");
        assert!(ended.is_some());
    }

    /// The escalation trigger. It has to count a budget stop as well: a run
    /// that ran out of budget did not finish the work either, and a task that
    /// keeps hitting the same ceiling must eventually be routed differently.
    #[test]
    fn only_unfinished_runs_of_the_same_prompt_count_towards_escalation() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("C:/a", "a").unwrap();
        let other = store.touch_project("C:/b", "b").unwrap();
        let close = |id: i64, prompt: &str, status: &str| {
            let task = store.create_task(new_task(id, prompt, "balanced")).unwrap();
            store.finish_task(task, status, "", "[]", 1).unwrap();
        };
        close(project.id, "do it", "failed");
        close(project.id, "do it", "budgetReached");
        close(project.id, "do it", "reviewRejected");
        close(project.id, "do it", "done");
        close(project.id, "do it", "cancelled");
        // A different prompt, and the same prompt in a different project.
        close(project.id, "do something else", "failed");
        close(other.id, "do it", "failed");
        // Ran out of plan usage: says nothing about the prompt, so not counted.
        let spent = store
            .create_task(new_task(project.id, "do it", "balanced"))
            .unwrap();
        store
            .append_event(
                spent,
                "run",
                "failed",
                "claude",
                r#"{"kind":"failed","data":{"kind":"usageLimit","message":"limit"}}"#,
            )
            .unwrap();
        store.finish_task(spent, "failed", "", "[]", 1).unwrap();

        assert_eq!(store.prior_failures(project.id, "do it").unwrap(), 3);
        assert_eq!(store.prior_failures(project.id, "never asked").unwrap(), 0);
    }

    /// No savings figure from a handful of runs, and none from runs that are
    /// not comparable: another route, provider, mode or project, or a run that
    /// did not finish. Cache reads never count as tokens spent.
    #[test]
    fn a_baseline_needs_five_comparable_finished_runs() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("C:/a", "a").unwrap();
        let other = store.touch_project("C:/b", "b").unwrap();
        let run_in =
            |mode: &str, project_id: i64, kind: &str, provider: &str, status: &str, tokens: u64| {
                let route = format!(r#"{{"kind":"{kind}"}}"#);
                let task = store
                    .create_task(NewTask {
                        route_json: Some(&route),
                        ..new_task(project_id, "p", mode)
                    })
                    .unwrap();
                let usage = Usage {
                    model: None,
                    input_tokens: tokens,
                    cached_input_tokens: tokens * 10,
                    output_tokens: 0,
                    reasoning_tokens: 0,
                    cost_usd: None,
                    cost_quality: CostQuality::Unavailable,
                };
                store
                    .record_usage(task, None, provider, Some(&usage))
                    .unwrap();
                store.finish_task(task, status, "", "[]", 1).unwrap();
                task
            };
        let run = |project_id: i64, kind: &str, provider: &str, status: &str, tokens: u64| {
            run_in("balanced", project_id, kind, provider, status, tokens)
        };
        for tokens in [100, 400, 200, 300] {
            run(project.id, "implementOnce", "codex", "done", tokens);
        }
        run(project.id, "implementOnce", "codex", "budgetReached", 1);
        run(project.id, "planned", "codex", "done", 1);
        run(project.id, "implementOnce", "claude", "done", 1);
        run(other.id, "implementOnce", "codex", "done", 1);
        run_in("efficient", project.id, "implementOnce", "codex", "done", 1);
        let current = store
            .create_task(new_task(project.id, "p", "balanced"))
            .unwrap();
        assert_eq!(
            store.baseline(current, "implementOnce", "codex").unwrap(),
            None
        );

        run(project.id, "implementOnce", "codex", "done", 500);
        assert_eq!(
            store.baseline(current, "implementOnce", "codex").unwrap(),
            Some(Baseline {
                runs: 5,
                median_tokens: 300,
                median_calls: 1
            })
        );
    }

    /// Two stalls in five finished runs of one route kind, tier and provider.
    /// Failures and other providers do not count, and old evidence ages out.
    #[test]
    fn a_tier_stalls_on_two_in_five_recent_finished_runs() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("C:/a", "a").unwrap();
        let run = |kind: &str, tier: &str, provider: &str, status: &str| {
            let route = format!(r#"{{"kind":"{kind}","budget":{{"preferredTier":"{tier}"}}}}"#);
            let task = store
                .create_task(NewTask {
                    route_json: Some(&route),
                    ..new_task(project.id, "p", "balanced")
                })
                .unwrap();
            store.record_usage(task, None, provider, None).unwrap();
            store.finish_task(task, status, "", "[]", 1).unwrap();
            task
        };
        let stalled = || {
            store
                .stalled_tiers(project.id, "codex", "balanced")
                .unwrap()
        };
        for status in ["done", "done", "done", "budgetReached", "failed", "failed"] {
            run("implementOnce", "cheapest", "codex", status);
        }
        run("implementOnce", "cheapest", "claude", "reviewRejected");
        run("implementOnce", "standard", "codex", "budgetReached");
        assert_eq!(
            stalled(),
            Vec::new(),
            "one stall in four finished runs is not a pattern"
        );

        run("implementOnce", "cheapest", "codex", "reviewRejected");
        assert_eq!(stalled(), [(RouteKind::ImplementOnce, Tier::Cheapest)]);

        // A run that needed fixes and then finished did not stall.
        for _ in 0..5 {
            let task = run("planned", "standard", "codex", "done");
            store
                .append_event(task, "fix", "stage", "codex", "{}")
                .unwrap();
        }
        assert_eq!(stalled(), [(RouteKind::ImplementOnce, Tier::Cheapest)]);

        for _ in 0..50 {
            run("standard", "standard", "codex", "done");
        }
        assert_eq!(
            stalled(),
            Vec::new(),
            "evidence older than the last 50 runs ages out"
        );
    }

    #[test]
    fn a_task_cannot_be_opened_for_a_project_that_is_gone() {
        let store = Store::in_memory().unwrap();
        assert!(store.project("C:/never-opened").is_err());
    }

    #[test]
    fn trust_is_remembered() {
        let store = Store::in_memory().unwrap();
        let p = store.touch_project("C:/a", "a").unwrap();
        assert!(!p.trusted, "a new project starts untrusted");

        store.set_trusted("C:/a", true).unwrap();
        assert!(store.recent_projects(1).unwrap()[0].trusted);
    }
}
