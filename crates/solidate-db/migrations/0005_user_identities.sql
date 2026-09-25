-- External identities linked to users: an OpenID Connect issuer and the subject
-- it asserts. A user may have any number of identities and no password.

CREATE TABLE user_identities (
    issuer     text NOT NULL,
    subject    text NOT NULL,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (issuer, subject)
);
CREATE INDEX user_identities_user_idx ON user_identities (user_id);

GRANT SELECT, INSERT, UPDATE, DELETE ON user_identities TO solidate_app;
