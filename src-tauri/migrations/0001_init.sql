CREATE TABLE projects (
  id               INTEGER PRIMARY KEY,
  path             TEXT    NOT NULL UNIQUE,
  name             TEXT    NOT NULL,
  trusted          INTEGER NOT NULL DEFAULT 0,
  trust_scanned_at TEXT,
  last_opened_at   TEXT    NOT NULL,
  -- datetime('now') only resolves to the second, so two opens in the same
  -- second would tie. Order recents by this instead; the timestamp is display.
  opened_seq       INTEGER NOT NULL
);

CREATE INDEX projects_recent ON projects (opened_seq DESC);
