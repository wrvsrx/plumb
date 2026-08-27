DROP INDEX IF EXISTS tasks_due_order;
DROP INDEX IF EXISTS tasks_source_order;
ALTER TABLE tasks DROP COLUMN prev_text;
ALTER TABLE tasks DROP COLUMN recur_text;
ALTER TABLE tasks DROP COLUMN selection_end;
ALTER TABLE tasks DROP COLUMN selection_start;
