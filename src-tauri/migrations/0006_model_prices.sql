-- Published API rates in USD per million tokens, replaced whole on each fetch.
CREATE TABLE model_prices (
  model       TEXT PRIMARY KEY,
  input       REAL NOT NULL,
  output      REAL NOT NULL,
  cache_read  REAL NOT NULL,
  fetched_at  TEXT NOT NULL
);
