BEGIN;

-- # Media

CREATE TABLE storage_locations (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    recurse BOOLEAN NOT NULL
);

CREATE TABLE data_file (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL UNIQUE
);

CREATE TABLE content (
    id INTEGER PRIMARY KEY,
    last_changed INTEGER NOT NULL,
    hash BLOB NOT NULL,
    data_id INTEGER, -- Reference to a data_file id, is null when the data_file was invalidated
    type INTEGER NOT NULL, -- ContentType
    reference INTEGER, -- The key to another table based on type
    part INTEGER NOT NULL
);

CREATE TABLE content_playlist (
    content_id INTEGER REFERENCES content (id),
    stream_index INTEGER NOT NULL,
    playlist BLOB NOT NULL,
    UNIQUE (content_id) ON CONFLICT IGNORE
);

------------

-- # Content type data

CREATE TABLE movie (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL
);

CREATE TABLE episode (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    episode INTEGER NOT NULL
);

CREATE TABLE song (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL
);
------------

-- # Collections

CREATE TABLE collection (
    id INTEGER PRIMARY KEY,
    type INTEGER NOT NULL, -- CollectionType
    reference INTEGER NOT NULL -- The key to another table based on type
);

CREATE TABLE collection_contains (
    collection_id INTEGER REFERENCES collection (id), -- TODO: This fails somewhere!
    type INTEGER NOT NULL, -- TableId
    reference INTEGER, -- Either a collection or content
    UNIQUE (collection_id, type, reference) ON CONFLICT IGNORE
);

------------

-- # Collection data
CREATE TABLE franchise (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL
);

CREATE TABLE season (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL,
    season INTEGER NOT NULL
);

CREATE TABLE series (
    id INTEGER PRIMARY KEY,
    title TEXT NULL
);

CREATE TABLE theme (
    id INTEGER PRIMARY KEY,
    type INTEGER NOT NULL, -- TableId
    theme_target INTEGER -- Either a collection or content
);

------------

COMMIT;
