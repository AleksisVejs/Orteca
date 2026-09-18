-- Trees whose checks passed before a run (see run::verify_locally), so an app
-- restart does not rerun the suite on an unchanged tree.
CREATE TABLE check_passes (
  key        TEXT PRIMARY KEY,
  passed_at  TEXT NOT NULL
);
