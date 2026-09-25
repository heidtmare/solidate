-- Passwords move off `users` into their own table. A user without a row here has
-- no password and cannot sign in with one (e.g. accounts signed in through an
-- external identity provider). Reading a user no longer loads a secret.

CREATE TABLE password_credentials (
    user_id    uuid PRIMARY KEY REFERENCES users (id) ON DELETE CASCADE,
    hash       text NOT NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
);

INSERT INTO password_credentials (user_id, hash) SELECT id, password_hash FROM users;

ALTER TABLE users DROP COLUMN password_hash;

GRANT SELECT, INSERT, UPDATE, DELETE ON password_credentials TO solidate_app;
