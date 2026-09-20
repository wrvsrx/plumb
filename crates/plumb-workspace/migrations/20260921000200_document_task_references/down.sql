CREATE TABLE task_dependencies_old (
    source_path BLOB NOT NULL,
    source_start BIGINT NOT NULL,
    source_id TEXT,
    target_path BLOB NOT NULL,
    target_id TEXT NOT NULL,
    source_text TEXT NOT NULL
);
INSERT INTO task_dependencies_old (source_path, source_start, source_id, target_path, target_id, source_text) SELECT source_path, source_start, source_id, target_path, target_id, source_text FROM task_dependencies WHERE target_id IS NOT NULL;
DROP TABLE task_dependencies;
ALTER TABLE task_dependencies_old RENAME TO task_dependencies;
CREATE INDEX task_dependencies_source ON task_dependencies(source_path, source_start);
CREATE INDEX task_dependencies_target ON task_dependencies(target_path, target_id);
CREATE TABLE event_task_associations_old (
    source_path BLOB NOT NULL,
    event_start BIGINT NOT NULL,
    target_path BLOB NOT NULL,
    target_id TEXT NOT NULL,
    source_text TEXT NOT NULL,
    source_start BIGINT NOT NULL,
    source_end BIGINT NOT NULL
);
INSERT INTO event_task_associations_old (source_path, event_start, target_path, target_id, source_text, source_start, source_end) SELECT source_path, event_start, target_path, target_id, source_text, source_start, source_end FROM event_task_associations WHERE target_id IS NOT NULL;
DROP TABLE event_task_associations;
ALTER TABLE event_task_associations_old RENAME TO event_task_associations;
CREATE INDEX event_task_associations_event ON event_task_associations(source_path, event_start);
CREATE INDEX event_task_associations_target ON event_task_associations(target_path, target_id, source_path, event_start);
