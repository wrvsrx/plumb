DROP INDEX IF EXISTS events_source_order;
ALTER TABLE events DROP COLUMN depth;
ALTER TABLE events DROP COLUMN selection_end;
ALTER TABLE events DROP COLUMN selection_start;
ALTER TABLE events DROP COLUMN id;
