ALTER TABLE tasks ADD COLUMN selection_start BIGINT NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN selection_end BIGINT NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN recur_text TEXT;
ALTER TABLE tasks ADD COLUMN prev_text TEXT;

CREATE INDEX tasks_source_order ON tasks(path, start);
CREATE INDEX tasks_due_order ON tasks(due_millis, path, start);
