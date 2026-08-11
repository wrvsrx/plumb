ALTER TABLE tasks ADD COLUMN closure_state TEXT NOT NULL DEFAULT 'open';
ALTER TABLE tasks ADD COLUMN created_millis BIGINT;
ALTER TABLE tasks ADD COLUMN due_millis BIGINT;
ALTER TABLE tasks ADD COLUMN wait_millis BIGINT;
ALTER TABLE tasks ADD COLUMN done_millis BIGINT;
ALTER TABLE tasks ADD COLUMN canceled_millis BIGINT;
ALTER TABLE tasks ADD COLUMN priority INTEGER;
ALTER TABLE tasks ADD COLUMN depth BIGINT NOT NULL DEFAULT 0;
ALTER TABLE tasks ADD COLUMN parent_start BIGINT;

CREATE INDEX tasks_state_due
    ON tasks(closure_state, due_millis, path, start);
CREATE INDEX tasks_priority
    ON tasks(priority DESC, path, start);

CREATE TABLE task_dependencies (
    source_path BLOB NOT NULL,
    source_start BIGINT NOT NULL,
    source_id TEXT,
    target_path BLOB NOT NULL,
    target_id TEXT NOT NULL,
    source_text TEXT NOT NULL
);
CREATE INDEX task_dependencies_source
    ON task_dependencies(source_path, source_start);
CREATE INDEX task_dependencies_target
    ON task_dependencies(target_path, target_id);
