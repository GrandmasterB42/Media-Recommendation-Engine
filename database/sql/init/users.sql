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

------------

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

------------

COMMIT;