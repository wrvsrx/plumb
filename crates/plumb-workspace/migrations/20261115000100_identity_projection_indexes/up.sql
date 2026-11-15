CREATE INDEX tasks_ordered_identity
    ON tasks(path, document_task DESC, start, id, closure_state);
CREATE INDEX anchors_source_identity
    ON anchors(path, start, id);
