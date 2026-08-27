CREATE TABLE event_task_associations (
    source_path BLOB NOT NULL,
    event_start BIGINT NOT NULL,
    target_path BLOB NOT NULL,
    target_id TEXT NOT NULL,
    source_text TEXT NOT NULL,
    source_start BIGINT NOT NULL,
    source_end BIGINT NOT NULL
);
CREATE INDEX event_task_associations_event
    ON event_task_associations(source_path, event_start);
CREATE INDEX event_task_associations_target
    ON event_task_associations(target_path, target_id, source_path, event_start);
