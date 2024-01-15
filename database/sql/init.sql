BEGIN;
CREATE TABLE storage_locations (path);

INSERT INTO storage_locations VALUES ('Y:');

CREATE TABLE data_files (
    id INTEGER PRIMARY KEY,
    path TEXT NOT NULL
);

CREATE TABLE multipart (
    id INTEGER NOT NULL,
    videoid INTEGER REFERENCES data_files (id),
    part INTEGER NOT NULL
);

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