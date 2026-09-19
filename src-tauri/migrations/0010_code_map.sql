-- What each tracked source file defines and uses, for ranking the files a
-- task is about. A use is a bare name, resolved to files through map_symbols
-- when read, so a changed file only rewrites its own rows. A file is parsed
-- again only when its mtime or size moves.
CREATE TABLE map_files (
  id         INTEGER PRIMARY KEY,
  project_id INTEGER NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  path       TEXT    NOT NULL,
  mtime      INTEGER NOT NULL,
  size       INTEGER NOT NULL,
  lang       TEXT    NOT NULL,
  UNIQUE (project_id, path)
);

CREATE TABLE map_symbols (
  file_id INTEGER NOT NULL REFERENCES map_files(id) ON DELETE CASCADE,
  name    TEXT    NOT NULL,
  kind    TEXT    NOT NULL,
  line    INTEGER NOT NULL
);

CREATE INDEX map_symbols_file ON map_symbols (file_id);

CREATE TABLE map_uses (
  file_id INTEGER NOT NULL REFERENCES map_files(id) ON DELETE CASCADE,
  name    TEXT    NOT NULL
);

CREATE INDEX map_uses_file ON map_uses (file_id);
