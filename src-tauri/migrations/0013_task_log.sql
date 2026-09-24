-- How each run was set up and how it ended, so models, rulesets and tiers can
-- be compared on real tasks. NULL on rows from before this was recorded.
ALTER TABLE tasks ADD COLUMN task_type TEXT;          -- chat | question | code_change | debug | plan
ALTER TABLE tasks ADD COLUMN ruleset TEXT;            -- name@version
ALTER TABLE tasks ADD COLUMN turns INTEGER;
ALTER TABLE tasks ADD COLUMN tools_before_edit INTEGER;
ALTER TABLE tasks ADD COLUMN gate TEXT;               -- pass | fail | none
ALTER TABLE tasks ADD COLUMN verdict TEXT;            -- accepted | rejected | retried

-- The user's own wording of the repository profile. NULL sends the detected one.
ALTER TABLE projects ADD COLUMN profile TEXT;

-- Files an agent read or edited in a task, which rank the files of later tasks.
CREATE TABLE task_files (
  task_id INTEGER NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  path    TEXT    NOT NULL,
  edited  INTEGER NOT NULL,
  PRIMARY KEY (task_id, path)
);

-- Each window-anchoring ping and what it cost.
CREATE TABLE pings (
  id            INTEGER PRIMARY KEY,
  provider      TEXT    NOT NULL,
  at            TEXT    NOT NULL DEFAULT (datetime('now')),
  input_tokens  INTEGER,
  output_tokens INTEGER,
  ok            INTEGER NOT NULL
);
