BEGIN;
DELETE FROM multipart;
DELETE FROM data_files;
DELETE FROM episodes;
DELETE FROM seasons;
DELETE FROM series;
DELETE FROM movies;
DELETE FROM franchise;
COMMIT;