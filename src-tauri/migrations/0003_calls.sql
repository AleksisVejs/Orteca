-- Provider processes a task started, exact. NULL for tasks finished before this
-- was recorded, which therefore never join a baseline.
ALTER TABLE tasks ADD COLUMN calls_used INTEGER;
