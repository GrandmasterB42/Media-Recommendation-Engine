BEGIN;
DELETE FROM data_file;
DELETE FROM content;
DELETE FROM movie;
DELETE FROM episode;
DELETE FROM song;
DELETE FROM franchise;
DELETE FROM season;
DELETE FROM series;
DELETE FROM theme;
DELETE FROM collection;
DELETE FROM collection_contains;
COMMIT;