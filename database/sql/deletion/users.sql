BEGIN;
DELETE FROM user_groups;
DELETE FROM user_permissions;
DELETE FROM users;
COMMIT;