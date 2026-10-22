CREATE TABLE tag_owners (path BLOB NOT NULL, owner_kind INTEGER NOT NULL, start BIGINT NOT NULL, record BLOB NOT NULL, PRIMARY KEY(path, owner_kind, start));
