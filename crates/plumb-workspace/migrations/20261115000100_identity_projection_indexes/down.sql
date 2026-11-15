DROP INDEX anchors_source_identity;
DROP INDEX tasks_ordered_identity;
DROP INDEX anchors_identity;
CREATE INDEX anchors_identity ON anchors(path, id);
