CREATE TABLE IF NOT EXISTS cache_meta (
    key TEXT PRIMARY KEY NOT NULL,
    value BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS documents (
    path BLOB PRIMARY KEY NOT NULL,
    revision BIGINT NOT NULL,
    content_hash BLOB NOT NULL,
    valid BOOLEAN NOT NULL,
    title TEXT NOT NULL,
    title_start BIGINT NOT NULL,
    title_end BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS anchors (
    path BLOB NOT NULL,
    id TEXT NOT NULL,
    start BIGINT NOT NULL,
    record BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS anchors_identity ON anchors(path, id);

CREATE TABLE IF NOT EXISTS links (
    path BLOB NOT NULL,
    start BIGINT NOT NULL,
    end BIGINT NOT NULL,
    record BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS links_source_range ON links(path, start, end);

CREATE TABLE IF NOT EXISTS semantic_references (
    source_path BLOB NOT NULL,
    target_path BLOB NOT NULL,
    target_id TEXT,
    source_start BIGINT NOT NULL,
    source_end BIGINT NOT NULL,
    path_start BIGINT,
    path_end BIGINT,
    id_start BIGINT,
    id_end BIGINT
);
CREATE INDEX IF NOT EXISTS references_target
    ON semantic_references(target_path, target_id);
CREATE INDEX IF NOT EXISTS references_source ON semantic_references(source_path);

CREATE TABLE IF NOT EXISTS tasks (
    path BLOB NOT NULL,
    id TEXT,
    title TEXT NOT NULL,
    start BIGINT NOT NULL,
    record BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS tasks_identity ON tasks(path, id);

CREATE TABLE IF NOT EXISTS events (
    path BLOB NOT NULL,
    title TEXT NOT NULL,
    start BIGINT NOT NULL,
    is_point BOOLEAN NOT NULL,
    sort_millis BIGINT,
    interval_start_millis BIGINT,
    interval_end_millis BIGINT,
    record BLOB NOT NULL
);
CREATE INDEX IF NOT EXISTS events_time
    ON events(interval_start_millis, interval_end_millis);
