ALTER TABLE tasks ADD COLUMN focused BOOL NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN focused_since_millis BIGINT;
ALTER TABLE tasks ADD COLUMN focus_valid BOOL NOT NULL DEFAULT 1;

CREATE INDEX tasks_focused_order ON tasks(focused, path, start);
