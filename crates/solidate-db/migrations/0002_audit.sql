-- Audit log: one row per mutating operation, written in the operation's
-- transaction. Append-only for solidate_app (no UPDATE or DELETE grant).
--
-- Actor columns carry no foreign keys so entries outlive deleted users and tokens.
-- actor_kind 'system' covers the CLI and internal jobs (both ids NULL). Entries are
-- listed newest first by id (UUIDv7, time-ordered) through the primary key.

CREATE TABLE audit_log (
    id             uuid NOT NULL,
    tenant_id      uuid NOT NULL DEFAULT current_tenant() REFERENCES tenants (id) ON DELETE CASCADE,
    at             timestamptz NOT NULL DEFAULT now(),
    actor_kind     text NOT NULL CHECK (actor_kind IN ('user', 'token', 'system')),
    actor_user_id  uuid,
    actor_token_id uuid,
    action         text NOT NULL,
    project_id     uuid,
    target         text,
    detail         jsonb NOT NULL DEFAULT '{}',
    PRIMARY KEY (tenant_id, id),
    CHECK ((actor_kind = 'user') = (actor_user_id IS NOT NULL)),
    CHECK ((actor_kind = 'token') = (actor_token_id IS NOT NULL))
);

ALTER TABLE audit_log ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON audit_log TO solidate_app
    USING (tenant_id = current_tenant()) WITH CHECK (tenant_id = current_tenant());
GRANT SELECT, INSERT ON audit_log TO solidate_app;
