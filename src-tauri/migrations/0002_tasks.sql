CREATE TABLE tasks (
  id             INTEGER PRIMARY KEY,
  project_id     INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  prompt         TEXT    NOT NULL,
  mode           TEXT    NOT NULL,
  -- NULL until routing lands; a single-stage run has no route to record.
  route_json     TEXT,
  status         TEXT    NOT NULL,   -- running | done | cancelled | failed | budgetReached | reviewRejected
  branch         TEXT,
  base_commit    TEXT,
  dirty_at_start INTEGER NOT NULL DEFAULT 0,
  started_at     TEXT    NOT NULL,
  ended_at       TEXT,
  summary        TEXT,
  diff_stat_json TEXT
);

CREATE INDEX tasks_recent ON tasks (project_id, id DESC);

-- Append-only. One row per normalised ProviderEvent, payload as it was sent
-- to the UI, so the stream can be replayed without re-parsing a provider.
CREATE TABLE task_events (
  id           INTEGER PRIMARY KEY,
  task_id      INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  ts           TEXT    NOT NULL,
  stage        TEXT,
  kind         TEXT    NOT NULL,
  provider     TEXT    NOT NULL,
  payload_json TEXT    NOT NULL
);

CREATE INDEX task_events_task ON task_events (task_id, id);

-- Token counts are nullable on purpose. A run that dies before its provider
-- reports usage has no honest number, and a zero would read as "this was free".
CREATE TABLE usage (
  id                  INTEGER PRIMARY KEY,
  task_id             INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  event_id            INTEGER REFERENCES task_events(id),
  provider            TEXT    NOT NULL,
  model               TEXT,
  input_tokens        INTEGER,
  cached_input_tokens INTEGER,
  output_tokens       INTEGER,
  reasoning_tokens    INTEGER,
  cost_usd            REAL,
  cost_quality        TEXT    NOT NULL   -- exact | estimated | unavailable
);

CREATE INDEX usage_task ON usage (task_id);
