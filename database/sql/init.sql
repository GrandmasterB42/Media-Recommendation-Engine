BEGIN;

-- # Auth
CREATE TABLE session_store (
    id TEXT PRIMARY KEY NOT NULL,
    data BLOB NOT NULL,
    expiry_date INTEGER NOT NULL
);

CREATE TABLE users (
    id INTEGER PRIMARY KEY,
    username TEXT NOT NULL,
    password TEXT NOT NULL
);

-- # Permissions

CREATE TABLE permissions (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL
);

INSERT INTO permissions (name) VALUES ("owner");

CREATE TABLE groups (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE user_permissions (
    userid INTEGER REFERENCES users (id),
    permissionid INTEGER REFERENCES permissions (id),
    PRIMARY KEY (userid, permissionid)
);

CREATE TABLE user_groups (
    userid INTEGER REFERENCES users (id),
    groupid INTEGER REFERENCES groups (id),
    PRIMARY KEY (userid, groupid)
);

CREATE TABLE group_permissions (
    groupid INTEGER REFERENCES groups (id),
    permissionid INTEGER REFERENCES permissions (id),
    PRIMARY KEY (groupid, permissionid)
);

-- # Media
CREATE TABLE storage_locations (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL,
    recurse BOOLEAN NOT NULL
);

CREATE TABLE data_files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL
);

CREATE TABLE multipart (
    id INTEGER NOT NULL,
    videoid INTEGER REFERENCES data_files (id),
    part INTEGER NOT NULL
);

-- # Metadata
CREATE TABLE franchise (
    id INTEGER PRIMARY KEY,
    title TEXT NOT NULL
);

CREATE TABLE movies (
    id INTEGER PRIMARY KEY,
    franchiseid INTEGER REFERENCES franchise (id),
    videoid INTEGER,
    referenceflag INTEGER NOT NULL,
    title TEXT NOT NULL
);

CREATE TABLE series (
    id INTEGER PRIMARY KEY,
    franchiseid INTEGER REFERENCES franchise (id),
    title TEXT NULL
);

CREATE TABLE seasons (
    id INTEGER PRIMARY KEY,
    seriesid INTEGER REFERENCES series (id),
    season INTEGER NULL,
    title TEXT NULL
);

CREATE TABLE episodes (
    id INTEGER PRIMARY KEY,
    seasonid INTEGER REFERENCES seasons (id),
    videoid INTEGER,
    referenceflag INTEGER NOT NULL,
    episode INTEGER NOT NULL,
    title TEXT NULL
);

COMMIT;