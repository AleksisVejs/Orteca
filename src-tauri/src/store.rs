//! SQLite. Migrations are numbered SQL files applied in order, tracked with
//! `PRAGMA user_version`. No ORM, no query builder.

use std::collections::{HashMap, HashSet};
use std::os::windows::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection};
use serde::Serialize;

use crate::codemap;
use crate::error::{AppError, ErrorKind, Result};
use crate::project::FileStat;
use crate::providers::{CostQuality, Usage};
use crate::routing::{PastNote, RouteKind, Tier};

/// `text` cut to at most `max` characters, with an ellipsis when it was cut.
fn cut(text: &str, max: usize) -> String {
    match text.char_indices().nth(max) {
        Some((at, _)) => format!("{}…", &text[..at]),
        None => text.to_string(),
    }
}

const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/0001_init.sql"),
    include_str!("../migrations/0002_tasks.sql"),
    include_str!("../migrations/0003_calls.sql"),
    include_str!("../migrations/0004_task_details.sql"),
    include_str!("../migrations/0005_worktree.sql"),
    include_str!("../migrations/0006_model_prices.sql"),
    include_str!("../migrations/0007_task_titles.sql"),
    include_str!("../migrations/0008_check_passes.sql"),
    include_str!("../migrations/0009_drop_check_passes.sql"),
    include_str!("../migrations/0010_code_map.sql"),
    include_str!("../migrations/0011_memory.sql"),
];

/// Files past this are minified or generated, not something a task edits.
const MAP_MAX_BYTES: u64 = 256 * 1024;

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
    pub title: &'a str,
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
        // Every streamed event is its own insert. Under WAL, NORMAL syncs at
        // checkpoints instead of on each commit and still cannot corrupt the
        // file; a power cut costs at most the last few events.
        conn.pragma_update(None, "synchronous", "NORMAL")?;
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
                (project_id, prompt, title, mode, route_json, status, branch,
                 base_commit, dirty_at_start, started_at)
             VALUES (?1, ?2, ?3, ?4, ?5, 'running', ?6, ?7, ?8, datetime('now'))",
            params![
                task.project_id,
                task.prompt,
                task.title,
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
        // The hottest statement in the app: one per streamed event.
        conn.prepare_cached(
            "INSERT INTO task_events (task_id, ts, stage, kind, provider, payload_json)
             VALUES (?1, datetime('now'), ?2, ?3, ?4, ?5)",
        )?
        .execute(params![task_id, stage, kind, provider, payload_json])?;
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

    /// This provider has run a model before, and never this one. Asked before
    /// the run's usage is recorded; the first model a provider runs is not news.
    pub fn is_new_model(&self, provider: &str, model: &str) -> Result<bool> {
        let conn = self.0.lock().expect("store poisoned");
        Ok(conn.query_row(
            "SELECT EXISTS (SELECT 1 FROM usage WHERE provider = ?1 AND model IS NOT NULL)
                AND NOT EXISTS (SELECT 1 FROM usage WHERE provider = ?1 AND model = ?2)",
            params![provider, model],
            |r| r.get(0),
        )?)
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
    /// shows absolute numbers until there is more. A continued task (a `turn`
    /// event) is left out here and in `stalled_tiers`: its totals and status
    /// span turns on different routes, and its row keeps only the last one.
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
                AND NOT EXISTS (SELECT 1 FROM task_events e WHERE e.task_id = t.id AND e.kind = 'turn')
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
    /// stall: fixing until the checks pass is how a route works. Only runs on
    /// the model a route kind and tier last ran on count: when an alias moves
    /// to a new model, that model starts without the old one's record.
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
            "WITH runs AS (
                SELECT json_extract(t.route_json, '$.kind') AS kind,
                       json_extract(t.route_json, '$.budget.preferredTier') AS tier,
                       t.id, t.status, u.model
                  FROM tasks t JOIN usage u ON u.task_id = t.id
                 WHERE t.id IN (SELECT id FROM tasks WHERE project_id = ?1 ORDER BY id DESC LIMIT 50)
                   AND u.provider = ?2
                   AND t.mode = ?3
                   AND t.status IN ('done', 'budgetReached', 'reviewRejected', 'verifyFailed')
                   AND NOT EXISTS (SELECT 1 FROM task_events e WHERE e.task_id = t.id AND e.kind = 'turn'))
             SELECT kind, tier FROM (
                SELECT *, FIRST_VALUE(model) OVER (PARTITION BY kind, tier ORDER BY id DESC) AS latest
                  FROM runs)
              WHERE model IS latest
              GROUP BY kind, tier
             HAVING COUNT(*) >= 5
                AND 5 * SUM(status != 'done') >= 2 * COUNT(*)",
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

    /// Global items first, then this project's, oldest first in each. `None`
    /// lists the global items alone, for the screen before a project is open.
    pub fn memory(&self, project_id: Option<i64>) -> Result<MemoryState> {
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, project_id IS NULL, text FROM memory
              WHERE project_id IS NULL OR project_id = ?1
              ORDER BY project_id IS NOT NULL, id",
        )?;
        let items = stmt
            .query_map([project_id], |r| Ok(MemoryItem { id: r.get(0)?, global: r.get(1)?, text: r.get(2)? }))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let tokens = tokens_of(items.iter().map(|i| i.text.as_str()));
        Ok(MemoryState { items, limit: memory_limit(&conn)?, tokens })
    }

    /// What a run of this task is told: the memory items, global then project.
    pub fn memory_for_task(&self, task_id: i64) -> Result<Vec<String>> {
        let project_id: i64 =
            self.0.lock().expect("store poisoned").query_row("SELECT project_id FROM tasks WHERE id = ?1", [task_id], |r| r.get(0))?;
        Ok(self.memory(Some(project_id))?.items.into_iter().map(|i| i.text).collect())
    }

    /// `project_id` None adds a global item.
    pub fn add_memory(&self, project_id: Option<i64>, text: &str) -> Result<()> {
        let text = clean_memory(text)?;
        let conn = self.0.lock().expect("store poisoned");
        check_memory_fits(&conn, project_id, None, &text)?;
        conn.execute("INSERT INTO memory (project_id, text) VALUES (?1, ?2)", params![project_id, text])?;
        Ok(())
    }

    pub fn edit_memory(&self, id: i64, text: &str) -> Result<()> {
        let text = clean_memory(text)?;
        let conn = self.0.lock().expect("store poisoned");
        let project_id: Option<i64> = conn
            .query_row("SELECT project_id FROM memory WHERE id = ?1", [id], |r| r.get(0))
            .map_err(|_| AppError::new(ErrorKind::NotFound, "that memory item is gone"))?;
        check_memory_fits(&conn, project_id, Some(id), &text)?;
        conn.execute("UPDATE memory SET text = ?2 WHERE id = ?1", params![id, text])?;
        Ok(())
    }

    pub fn delete_memory(&self, id: i64) -> Result<()> {
        self.0.lock().expect("store poisoned").execute("DELETE FROM memory WHERE id = ?1", [id])?;
        Ok(())
    }

    /// 0 means no limit.
    pub fn set_memory_limit(&self, limit: u32) -> Result<()> {
        self.0.lock().expect("store poisoned").execute(
            "INSERT INTO settings (key, value) VALUES ('memory_limit', ?1)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            [limit.to_string()],
        )?;
        Ok(())
    }

    /// Bring a project's code map up to date with its tracked files. Only a
    /// file whose mtime or size moved is read and parsed again, and a file no
    /// longer tracked loses its rows. Parsing happens outside the lock. Returns
    /// how many files were parsed.
    pub fn scan_map(&self, project_id: i64, dir: &Path, tracked: &[String]) -> Result<usize> {
        let known: HashMap<String, (i64, i64)> = {
            let conn = self.0.lock().expect("store poisoned");
            let mut stmt = conn.prepare("SELECT path, mtime, size FROM map_files WHERE project_id = ?1")?;
            let rows = stmt.query_map([project_id], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
            rows.collect::<rusqlite::Result<_>>()?
        };
        let mut kept: HashSet<&str> = HashSet::new();
        let mut parsed = Vec::new();
        for path in tracked {
            let Some(lang) = codemap::Lang::of(path) else { continue };
            if path.contains(".min.") || path.split('/').any(|d| d == "vendor" || d == "node_modules") {
                continue;
            }
            let full = dir.join(path);
            let Ok(meta) = std::fs::metadata(&full) else { continue };
            if !meta.is_file() || meta.len() > MAP_MAX_BYTES {
                continue;
            }
            let mtime = meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_nanos() as i64);
            let stamp = (mtime, meta.len() as i64);
            kept.insert(path);
            if known.get(path) == Some(&stamp) {
                continue;
            }
            let Ok(bytes) = std::fs::read(&full) else { continue };
            let facts = codemap::parse(lang, &String::from_utf8_lossy(&bytes));
            parsed.push((path, lang, stamp, facts));
        }

        let mut conn = self.0.lock().expect("store poisoned");
        let tx = conn.transaction()?;
        let clear = |path: &str| -> rusqlite::Result<()> {
            let file = "SELECT id FROM map_files WHERE project_id = ?1 AND path = ?2";
            tx.execute(&format!("DELETE FROM map_symbols WHERE file_id IN ({file})"), params![project_id, path])?;
            tx.execute(&format!("DELETE FROM map_uses WHERE file_id IN ({file})"), params![project_id, path])?;
            Ok(())
        };
        for gone in known.keys().filter(|p| !kept.contains(p.as_str())) {
            clear(gone)?;
            tx.execute("DELETE FROM map_files WHERE project_id = ?1 AND path = ?2", params![project_id, gone])?;
        }
        for (path, lang, (mtime, size), facts) in &parsed {
            clear(path)?;
            let id: i64 = tx.query_row(
                "INSERT INTO map_files (project_id, path, mtime, size, lang) VALUES (?1, ?2, ?3, ?4, ?5)
                 ON CONFLICT(project_id, path) DO UPDATE SET
                   mtime = excluded.mtime, size = excluded.size, lang = excluded.lang
                 RETURNING id",
                params![project_id, path, mtime, size, lang.name()],
                |r| r.get(0),
            )?;
            // Cached: a first scan inserts tens of thousands of rows, and
            // re-preparing each one was most of its time.
            let mut symbol = tx.prepare_cached("INSERT INTO map_symbols (file_id, name, kind, line) VALUES (?1, ?2, ?3, ?4)")?;
            for s in &facts.defines {
                symbol.execute(params![id, s.name, s.kind, s.line])?;
            }
            let mut uses = tx.prepare_cached("INSERT INTO map_uses (file_id, name) VALUES (?1, ?2)")?;
            for name in &facts.uses {
                uses.execute(params![id, name])?;
            }
        }
        tx.commit()?;
        Ok(parsed.len())
    }

    /// Every mapped file of a project, by path.
    pub fn code_map(&self, project_id: i64) -> Result<Vec<(String, codemap::FileFacts)>> {
        let conn = self.0.lock().expect("store poisoned");
        let mut files: Vec<(i64, String, codemap::FileFacts)> = conn
            .prepare("SELECT id, path FROM map_files WHERE project_id = ?1 ORDER BY path")?
            .query_map([project_id], |r| Ok((r.get(0)?, r.get(1)?, Default::default())))?
            .collect::<rusqlite::Result<_>>()?;
        let at: HashMap<i64, usize> = files.iter().enumerate().map(|(i, f)| (f.0, i)).collect();
        let mut stmt = conn.prepare(
            "SELECT s.file_id, s.name, s.kind, s.line FROM map_symbols s
               JOIN map_files f ON f.id = s.file_id WHERE f.project_id = ?1 ORDER BY s.file_id, s.line",
        )?;
        for row in stmt.query_map([project_id], |r| {
            Ok((r.get::<_, i64>(0)?, codemap::Symbol { name: r.get(1)?, kind: r.get(2)?, line: r.get(3)? }))
        })? {
            let (file, symbol) = row?;
            files[at[&file]].2.defines.push(symbol);
        }
        let mut stmt = conn.prepare(
            "SELECT u.file_id, u.name FROM map_uses u
               JOIN map_files f ON f.id = u.file_id WHERE f.project_id = ?1 ORDER BY u.file_id, u.name",
        )?;
        for row in stmt.query_map([project_id], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))? {
            let (file, name) = row?;
            files[at[&file]].2.uses.push(name);
        }
        Ok(files.into_iter().map(|(_, path, facts)| (path, facts)).collect())
    }

    /// This project's last 200 finished tasks as short factual notes: what was
    /// asked, what came back, which files changed. Choosing among them is
    /// `routing`'s job, on the files the new prompt is about.
    // ponytail: the last 200 done tasks; an index when a project outgrows that.
    pub fn past_notes(&self, project_id: i64) -> Result<Vec<PastNote>> {
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT id, prompt, summary, diff_stat_json FROM tasks
              WHERE project_id = ?1 AND status = 'done' ORDER BY id DESC LIMIT 200",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut notes = Vec::new();
        for row in rows {
            let (id, asked, summary, diff) = row?;
            let summary = summary.unwrap_or_default();
            let files: Vec<String> = diff
                .and_then(|d| serde_json::from_str::<Vec<FileStat>>(&d).ok())
                .unwrap_or_default()
                .into_iter()
                .map(|f| f.path)
                .collect();
            let mut note = format!("Task: {}", cut(asked.trim(), 160));
            if !summary.trim().is_empty() {
                note.push_str(&format!("\n  Result: {}", cut(summary.trim(), 300)));
            }
            if !files.is_empty() {
                let shown: Vec<&str> = files.iter().take(6).map(String::as_str).collect();
                note.push_str(&format!("\n  Files: {}", shown.join(", ")));
            }
            notes.push(PastNote { id, note, files });
        }
        Ok(notes)
    }

    /// This project's last 30 settled runs as one digest for memory
    /// suggestions: the user's own words (a reply's, not the exchange it
    /// quotes), how the run ended, and what came back. Empty with no runs.
    pub fn run_digest(&self, project_id: i64) -> Result<String> {
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT prompt, status, summary FROM tasks
              WHERE project_id = ?1 AND status != 'running' ORDER BY id DESC LIMIT 30",
        )?;
        let rows = stmt.query_map([project_id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Option<String>>(2)?))
        })?;
        let mut digest = String::new();
        for row in rows {
            let (asked, status, summary) = row?;
            let said = asked.rsplit("My reply:").next().unwrap_or(&asked).trim();
            digest.push_str(&format!("- Asked: {}\n  Ended: {status}\n", cut(said, 300)));
            if let Some(summary) = summary.filter(|s| !s.trim().is_empty()) {
                digest.push_str(&format!("  Result: {}\n", cut(summary.trim(), 300)));
            }
        }
        Ok(digest)
    }

    /// A project's runs, newest first. Token counts stay NULL where the
    /// provider reported none, as they are in `usage`.
    pub fn recent_tasks(&self, project_id: i64, limit: u32) -> Result<Vec<TaskSummary>> {
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT t.id, t.prompt, t.title, t.status, t.started_at, t.summary,
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
                title: r.get(2)?, status: r.get(3)?, started_at: r.get(4)?, summary: r.get(5)?,
                route_kind: r.get(6)?, calls_used: r.get(7)?, provider: r.get(8)?, model: r.get(9)?,
                tokens: r.get(10)?, uncached_tokens: r.get(11)?, cached_tokens: r.get(12)?,
                cost_usd: r.get(13)?, cost_quality: r.get(14)?, unknown_events: r.get(15)?,
                duration_ms: r.get(16)?, patch_available: r.get(17)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Recent tasks across remembered projects, with their owners for the
    /// shared sidebar. Details remain project-scoped.
    pub fn global_tasks(&self, limit: u32) -> Result<Vec<GlobalTaskSummary>> {
        let conn = self.0.lock().expect("store poisoned");
        let mut stmt = conn.prepare(
            "SELECT t.id, t.prompt, t.title, t.status, t.started_at, t.summary,
                    json_extract(t.route_json, '$.kind'), t.calls_used, u.provider, u.model,
                    u.input_tokens + u.cached_input_tokens + u.output_tokens,
                    u.input_tokens + u.output_tokens, u.cached_input_tokens, u.cost_usd,
                    u.cost_quality, t.unknown_events, t.duration_ms,
                    (t.patch_text IS NOT NULL AND length(t.patch_text) > 0), p.path, p.name
               FROM tasks t JOIN projects p ON p.id = t.project_id
               LEFT JOIN usage u ON u.task_id = t.id
              ORDER BY t.id DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map([limit], |r| {
            Ok(GlobalTaskSummary {
                task: TaskSummary {
                    id: r.get(0)?, prompt: r.get(1)?, title: r.get(2)?, status: r.get(3)?,
                    started_at: r.get(4)?, summary: r.get(5)?, route_kind: r.get(6)?,
                    calls_used: r.get(7)?, provider: r.get(8)?, model: r.get(9)?,
                    tokens: r.get(10)?, uncached_tokens: r.get(11)?, cached_tokens: r.get(12)?,
                    cost_usd: r.get(13)?, cost_quality: r.get(14)?, unknown_events: r.get(15)?,
                    duration_ms: r.get(16)?, patch_available: r.get(17)?,
                },
                project_path: r.get(18)?, project_name: r.get(19)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn rename_task(&self, project_id: i64, task_id: i64, title: &str) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        let changed = conn.execute(
            "UPDATE tasks SET title = ?1 WHERE project_id = ?2 AND id = ?3",
            params![title, project_id, task_id],
        )?;
        if changed == 0 {
            return Err(AppError::new(ErrorKind::NotFound, "That task is not in this project."));
        }
        Ok(())
    }

    /// Events and usage go with it. A running task, or one whose copy folder
    /// still exists, stays: deleting the row would orphan the folder.
    pub fn delete_task(&self, project_id: i64, task_id: i64) -> Result<()> {
        let conn = self.0.lock().expect("store poisoned");
        let changed = conn.execute(
            "DELETE FROM tasks WHERE project_id = ?1 AND id = ?2
                AND status != 'running' AND worktree_path IS NULL",
            params![project_id, task_id],
        )?;
        if changed == 0 {
            return Err(AppError::new(
                ErrorKind::Invalid,
                "That task is still running or still has a copy folder. Remove the copy first.",
            ));
        }
        Ok(())
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
                    t.branch, t.worktree_path, u.input_tokens
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
                        input_tokens: r.get(22)?,
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
    pub title: String,
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

/// A task for the global sidebar, carrying its project's display identity.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GlobalTaskSummary {
    #[serde(flatten)]
    pub task: TaskSummary,
    pub project_path: String,
    pub project_name: String,
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
    /// Uncached input alone, for the cache-hit share; output is not input.
    pub input_tokens: Option<u64>,
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

const DEFAULT_MEMORY_LIMIT: u32 = 1000;

/// A memory item as the panel lists it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryItem {
    pub id: i64,
    pub global: bool,
    pub text: String,
}

/// What a run sends and what the user allowed. `tokens` is characters / 4,
/// an estimate, and counts the global and project items together.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MemoryState {
    pub items: Vec<MemoryItem>,
    pub limit: u32,
    pub tokens: u32,
}

fn tokens_of<'a>(texts: impl Iterator<Item = &'a str>) -> u32 {
    texts.map(|t| t.chars().count() as u32).sum::<u32>().div_ceil(4)
}

fn memory_limit(conn: &Connection) -> Result<u32> {
    let value: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = 'memory_limit'", [], |r| r.get(0))
        .ok();
    Ok(value.and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_MEMORY_LIMIT))
}

fn clean_memory(text: &str) -> Result<String> {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.is_empty() {
        return Err(AppError::new(ErrorKind::Invalid, "a memory item cannot be empty"));
    }
    Ok(text)
}

/// Refuse `text` when it would push what a run sends past the limit. A global
/// item is measured against the global list alone, a project item against
/// global plus its own project.
// ponytail: a global item can still push some project past the limit; the
// panel shows the true total, so it is visible, not silent.
fn check_memory_fits(conn: &Connection, project_id: Option<i64>, replacing: Option<i64>, text: &str) -> Result<()> {
    let limit = memory_limit(conn)?;
    if limit == 0 {
        return Ok(());
    }
    let mut stmt = conn.prepare("SELECT text FROM memory WHERE (project_id IS NULL OR project_id IS ?1) AND id IS NOT ?2")?;
    let mut all: Vec<String> = stmt
        .query_map(params![project_id, replacing], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    all.push(text.to_string());
    if tokens_of(all.iter().map(String::as_str)) > limit {
        return Err(AppError::new(
            ErrorKind::Invalid,
            "Memory is full. Shorten or remove something, or raise the limit.",
        ));
    }
    Ok(())
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
            title: "",
            mode,
            route_json: None,
            branch: None,
            base_commit: None,
            dirty_at_start: false,
        }
    }

    #[test]
    fn memory_keeps_scopes_apart_and_holds_the_limit() {
        let store = Store::in_memory().unwrap();
        let a = store.touch_project("C:/a", "a").unwrap();
        let b = store.touch_project("C:/b", "b").unwrap();
        store.add_memory(None, "global rule").unwrap();
        store.add_memory(Some(a.id), "rule for a").unwrap();
        store.add_memory(Some(b.id), "rule for b").unwrap();
        let texts = |id: Option<i64>| store.memory(id).unwrap().items.into_iter().map(|i| i.text).collect::<Vec<_>>();
        assert_eq!(texts(Some(a.id)), ["global rule", "rule for a"]);
        assert_eq!(texts(Some(b.id)), ["global rule", "rule for b"]);

        store.set_memory_limit(10).unwrap();
        let err = store.add_memory(Some(a.id), &"x".repeat(60)).unwrap_err();
        assert!(err.message.starts_with("Memory is full"));
        let id = store.memory(Some(a.id)).unwrap().items[1].id;
        assert!(store.edit_memory(id, &"x".repeat(60)).is_err());
        store.set_memory_limit(0).unwrap();
        store.add_memory(Some(a.id), &"x".repeat(60)).unwrap();
        assert!(store.memory(Some(a.id)).unwrap().tokens > 10);

        store.forget_project("C:/b").unwrap();
        assert_eq!(store.memory(Some(a.id)).unwrap().items.len(), 3);
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
    fn global_tasks_include_the_project_that_owns_each_task() {
        let store = Store::in_memory().unwrap();
        let first = store.touch_project("C:/first", "First").unwrap();
        let second = store.touch_project("C:/second", "Second").unwrap();
        let first_task = store.create_task(new_task(first.id, "first task", "balanced")).unwrap();
        let second_task = store.create_task(new_task(second.id, "second task", "balanced")).unwrap();
        store.finish_task(first_task, "done", "", "[]", 0).unwrap();
        store.finish_task(second_task, "done", "", "[]", 0).unwrap();

        let rows = store.global_tasks(20).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].task.id, second_task);
        assert_eq!((&rows[0].project_path, &rows[0].project_name), (&"C:/second".to_string(), &"Second".to_string()));
        assert_eq!(rows[1].task.id, first_task);
        assert_eq!((&rows[1].project_path, &rows[1].project_name), (&"C:/first".to_string(), &"First".to_string()));
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
    fn past_notes_are_this_projects_finished_work_with_its_files() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("a", "a").unwrap();
        let other = store.touch_project("b", "b").unwrap();
        let mine = store.create_task(new_task(project.id, "Fix duplicate sync", "balanced")).unwrap();
        store
            .finish_task_details(
                mine, "done", "Stopped a retry queueing the same UUID twice.",
                r#"[{"path":"src/offlineSync.ts","added":4,"deleted":1,"origin":null}]"#,
                None, 0, None, 1,
            )
            .unwrap();
        let running = store.create_task(new_task(project.id, "still going", "balanced")).unwrap();
        let theirs = store.create_task(new_task(other.id, "Other project", "balanced")).unwrap();
        store.finish_task_details(theirs, "done", "x", "[]", None, 0, None, 1).unwrap();
        let _ = running;

        let got = store.past_notes(project.id).unwrap();
        assert_eq!(got.len(), 1, "only this project's finished tasks");
        assert_eq!(got[0].files, ["src/offlineSync.ts"]);
        assert!(got[0].note.contains("same UUID") && got[0].note.contains("Files: src/offlineSync.ts"));
    }

    #[test]
    fn run_digest_keeps_the_users_own_words_and_skips_running_work() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("a", "a").unwrap();
        assert_eq!(store.run_digest(project.id).unwrap(), "", "no runs, nothing to suggest from");
        let reply = store
            .create_task(new_task(project.id, "Old ask\n\nYour answer:\nDone.\n\nMy reply:\nUse pnpm, not npm", "balanced"))
            .unwrap();
        store.finish_task_details(reply, "verifyFailed", "npm test failed", "[]", None, 0, None, 1).unwrap();
        store.create_task(new_task(project.id, "still going", "balanced")).unwrap();

        let digest = store.run_digest(project.id).unwrap();
        assert!(digest.contains("Asked: Use pnpm, not npm") && !digest.contains("Old ask"));
        assert!(digest.contains("Ended: verifyFailed") && digest.contains("Result: npm test failed"));
        assert!(!digest.contains("still going"));
    }

    #[test]
    fn tasks_rename_and_delete_only_in_their_project_and_once_settled() {
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("a", "a").unwrap();
        let other = store.touch_project("b", "b").unwrap();
        let task = store.create_task(new_task(project.id, "p", "balanced")).unwrap();

        store.rename_task(project.id, task, "New name").unwrap();
        assert_eq!(store.recent_tasks(project.id, 20).unwrap()[0].title, "New name");
        assert!(store.rename_task(other.id, task, "x").is_err());

        assert!(store.delete_task(project.id, task).is_err(), "still running");
        store.finish_task_details(task, "done", "", "[]", None, 0, None, 0).unwrap();
        assert!(store.delete_task(other.id, task).is_err());
        store.delete_task(project.id, task).unwrap();
        assert!(store.recent_tasks(project.id, 20).unwrap().is_empty());
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

        // A continued task spans routes, so it is no evidence for this one.
        let continued = run(project.id, "implementOnce", "codex", "done", 1);
        store.append_event(continued, "answer", "turn", "codex", "{}").unwrap();
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

        let continued = run("implementOnce", "cheapest", "codex", "reviewRejected");
        store.append_event(continued, "answer", "turn", "codex", "{}").unwrap();
        assert_eq!(stalled(), Vec::new(), "a continued task spans routes");

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

        let on = |model: &str, status: &str| {
            let task = run("planned", "deep", "codex", status);
            let usage = Usage {
                model: Some(model.into()),
                input_tokens: 0,
                cached_input_tokens: 0,
                output_tokens: 0,
                reasoning_tokens: 0,
                cost_usd: None,
                cost_quality: CostQuality::Unavailable,
            };
            store.record_usage(task, None, "codex", Some(&usage)).unwrap();
        };
        for status in ["done", "done", "done", "budgetReached", "budgetReached"] {
            on("old", status);
        }
        assert_eq!(stalled(), [(RouteKind::Planned, Tier::Deep)]);
        assert!(store.is_new_model("codex", "new").unwrap());
        assert!(!store.is_new_model("codex", "old").unwrap());
        on("new", "done");
        assert_eq!(stalled(), Vec::new(), "a new model starts with a clean record");
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

    #[test]
    fn code_map_rescans_only_what_changed() {
        let dir = std::env::temp_dir().join(format!("orteca-map-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("app")).unwrap();
        let write = |path: &str, text: &str| std::fs::write(dir.join(path), text).unwrap();
        write("app/Quote.php", "<?php class Quote {}");
        write("app/QuoteController.php", "<?php class QuoteController { function show() { Quote::find(1); } }");
        write("README.md", "not code");
        let store = Store::in_memory().unwrap();
        let project = store.touch_project("a", "a").unwrap();
        let tracked = |paths: &[&str]| paths.iter().map(|p| p.to_string()).collect::<Vec<_>>();
        let all = tracked(&["app/Quote.php", "app/QuoteController.php", "README.md"]);

        assert_eq!(store.scan_map(project.id, &dir, &all).unwrap(), 2);
        let map = store.code_map(project.id).unwrap();
        assert_eq!(map.len(), 2);
        let (path, facts) = &map[1];
        assert_eq!(path, "app/QuoteController.php");
        assert_eq!(facts.defines[0].name, "QuoteController");
        assert_eq!(facts.defines[1].kind, "method");
        assert_eq!(facts.uses, vec!["Quote"]);

        assert_eq!(store.scan_map(project.id, &dir, &all).unwrap(), 0, "nothing changed");

        write("app/Quote.php", "<?php class Quote { function total() {} }");
        assert_eq!(store.scan_map(project.id, &dir, &all).unwrap(), 1, "an edit is parsed again");
        assert_eq!(store.code_map(project.id).unwrap()[0].1.defines.len(), 2);

        // A rename is the old path gone and a new one tracked.
        std::fs::rename(dir.join("app/Quote.php"), dir.join("app/Offer.php")).unwrap();
        let renamed = tracked(&["app/Offer.php", "app/QuoteController.php"]);
        assert_eq!(store.scan_map(project.id, &dir, &renamed).unwrap(), 1);
        let paths: Vec<String> = store.code_map(project.id).unwrap().into_iter().map(|(p, _)| p).collect();
        assert_eq!(paths, vec!["app/Offer.php", "app/QuoteController.php"]);

        assert_eq!(store.scan_map(project.id, &dir, &tracked(&["app/Offer.php"])).unwrap(), 0);
        assert_eq!(store.code_map(project.id).unwrap().len(), 1, "a deleted file loses its rows");
        let orphans: i64 = store
            .0
            .lock()
            .unwrap()
            .query_row("SELECT COUNT(*) FROM map_uses", [], |r| r.get(0))
            .unwrap();
        assert_eq!(orphans, 0, "the deleted file's uses went with it");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
