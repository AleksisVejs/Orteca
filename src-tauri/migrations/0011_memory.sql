-- Short standing instructions the user writes, sent with every run. A NULL
-- project_id is global. Settings is a plain key/value pair, for the memory
-- limit today.
CREATE TABLE memory (
  id         INTEGER PRIMARY KEY,
  project_id INTEGER REFERENCES projects(id) ON DELETE CASCADE,
  text       TEXT    NOT NULL,
  created_at TEXT    NOT NULL DEFAULT (datetime('now'))
);

CREATE TABLE settings (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
