CREATE TABLE generation_clock (id INTEGER PRIMARY KEY CHECK (id = 1), sequence BIGINT NOT NULL, identity BLOB NOT NULL);
INSERT INTO generation_clock VALUES (1, 0, randomblob(16));
CREATE TABLE generation_changes (path BLOB PRIMARY KEY NOT NULL, sequence BIGINT NOT NULL);
CREATE INDEX generation_changes_sequence ON generation_changes(sequence);
CREATE TRIGGER document_generation_insert AFTER INSERT ON documents BEGIN
    UPDATE generation_clock SET sequence = sequence + 1 WHERE id = 1;
    INSERT INTO generation_changes(path, sequence) VALUES (NEW.path, (SELECT sequence FROM generation_clock WHERE id = 1))
    ON CONFLICT(path) DO UPDATE SET sequence = excluded.sequence;
END;
CREATE TRIGGER document_generation_delete AFTER DELETE ON documents BEGIN
    UPDATE generation_clock SET sequence = sequence + 1 WHERE id = 1;
    INSERT INTO generation_changes(path, sequence) VALUES (OLD.path, (SELECT sequence FROM generation_clock WHERE id = 1))
    ON CONFLICT(path) DO UPDATE SET sequence = excluded.sequence;
END;
