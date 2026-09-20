DROP INDEX IF EXISTS tasks_focused_order;
ALTER TABLE tasks DROP COLUMN focus_valid;
ALTER TABLE tasks DROP COLUMN focused_since_millis;
ALTER TABLE tasks DROP COLUMN focused;
