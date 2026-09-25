-- Solidate schema.
--
-- Tenant isolation: every tenant-scoped table has `tenant_id` (defaulting to
-- current_tenant()) and an RLS policy restricting role `solidate_app` to rows of
-- the tenant set via `SET LOCAL app.tenant_id`. With no tenant set, no rows match.
-- Application connections `SET ROLE solidate_app`; migrations run as the owner.
--
-- Cross-table references include tenant_id in composite foreign keys, so a row can
-- never reference another tenant's row.
--
-- Global tables (tenants, users, sessions) are not tenant-scoped. Lookups that must
-- happen before a tenant is known go through SECURITY DEFINER functions.

DO $$ BEGIN
    CREATE ROLE solidate_app NOLOGIN;
-- unique_violation: concurrent creation (e.g. parallel test databases).
EXCEPTION WHEN duplicate_object OR unique_violation THEN NULL;
END $$;

CREATE FUNCTION current_tenant() RETURNS uuid
    LANGUAGE sql STABLE
    AS $$ SELECT nullif(current_setting('app.tenant_id', true), '')::uuid $$;

-- Global ----------------------------------------------------------------------

CREATE TABLE tenants (
    id         uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    slug       text NOT NULL UNIQUE,
    name       text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE users (
    id            uuid PRIMARY KEY DEFAULT gen_random_uuid(),
    email         text NOT NULL,
    name          text NOT NULL,
    password_hash text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now(),
    disabled_at   timestamptz
);
CREATE UNIQUE INDEX users_email_key ON users (lower(email));

CREATE TABLE sessions (
    token_hash bytea PRIMARY KEY,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX sessions_user_idx ON sessions (user_id);

-- Tenant-scoped ---------------------------------------------------------------

CREATE TABLE memberships (
    tenant_id  uuid NOT NULL DEFAULT current_tenant() REFERENCES tenants (id) ON DELETE CASCADE,
    user_id    uuid NOT NULL REFERENCES users (id) ON DELETE CASCADE,
    role       text NOT NULL CHECK (role IN ('reader', 'editor', 'admin')),
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, user_id)
);

CREATE TABLE projects (
    id         uuid NOT NULL DEFAULT gen_random_uuid(),
    tenant_id  uuid NOT NULL DEFAULT current_tenant() REFERENCES tenants (id) ON DELETE CASCADE,
    slug       text NOT NULL,
    name       text NOT NULL,
    parent_id  uuid,
    settings   jsonb NOT NULL DEFAULT '{}',
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id),
    UNIQUE (id),
    UNIQUE (tenant_id, slug),
    FOREIGN KEY (tenant_id, parent_id) REFERENCES projects (tenant_id, id) ON DELETE RESTRICT,
    CHECK (parent_id IS DISTINCT FROM id)
);

CREATE TABLE blobs (
    tenant_id  uuid NOT NULL DEFAULT current_tenant() REFERENCES tenants (id) ON DELETE CASCADE,
    hash       bytea NOT NULL,
    content    text NOT NULL,
    byte_len   integer NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, hash)
);

CREATE TABLE documents (
    id            uuid NOT NULL DEFAULT gen_random_uuid(),
    tenant_id     uuid NOT NULL DEFAULT current_tenant() REFERENCES tenants (id) ON DELETE CASCADE,
    project_id    uuid NOT NULL,
    path          text NOT NULL,
    title         text,
    sync_enabled  boolean NOT NULL DEFAULT true,
    created_at    timestamptz NOT NULL DEFAULT now(),
    updated_at    timestamptz NOT NULL DEFAULT now(),
    deleted_at    timestamptz,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES projects (tenant_id, id) ON DELETE CASCADE
);
CREATE UNIQUE INDEX documents_path_key ON documents (tenant_id, project_id, path) WHERE deleted_at IS NULL;

CREATE TABLE api_tokens (
    id           uuid NOT NULL DEFAULT gen_random_uuid(),
    tenant_id    uuid NOT NULL DEFAULT current_tenant() REFERENCES tenants (id) ON DELETE CASCADE,
    name         text NOT NULL,
    prefix       text NOT NULL UNIQUE,
    secret_hash  bytea NOT NULL,
    scopes       text[] NOT NULL,
    project_id   uuid,
    created_by   uuid REFERENCES users (id) ON DELETE SET NULL,
    created_at   timestamptz NOT NULL DEFAULT now(),
    expires_at   timestamptz,
    last_used_at timestamptz,
    revoked_at   timestamptz,
    PRIMARY KEY (tenant_id, id),
    UNIQUE (id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES projects (tenant_id, id) ON DELETE CASCADE,
    CHECK (scopes <@ ARRAY['read', 'write', 'admin'])
);

CREATE TABLE revisions (
    id              uuid NOT NULL DEFAULT gen_random_uuid(),
    tenant_id       uuid NOT NULL DEFAULT current_tenant() REFERENCES tenants (id) ON DELETE CASCADE,
    document_id     uuid NOT NULL,
    variant         text NOT NULL CHECK (variant IN ('human', 'ai')),
    content_hash    bytea NOT NULL,
    semantic_hash   bytea NOT NULL,
    parent_id       uuid,
    author_user_id  uuid REFERENCES users (id) ON DELETE SET NULL,
    author_token_id uuid,
    message         text,
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id),
    UNIQUE (id),
    FOREIGN KEY (tenant_id, document_id) REFERENCES documents (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, content_hash) REFERENCES blobs (tenant_id, hash),
    FOREIGN KEY (tenant_id, parent_id) REFERENCES revisions (tenant_id, id),
    FOREIGN KEY (tenant_id, author_token_id) REFERENCES api_tokens (tenant_id, id)
);
CREATE INDEX revisions_history_idx ON revisions (tenant_id, document_id, variant, created_at DESC);

-- Current revision of each variant.
CREATE TABLE heads (
    tenant_id    uuid NOT NULL DEFAULT current_tenant(),
    document_id  uuid NOT NULL,
    variant      text NOT NULL CHECK (variant IN ('human', 'ai')),
    revision_id  uuid NOT NULL,
    content_hash bytea NOT NULL,
    title        text,
    search       tsvector NOT NULL,
    updated_at   timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, document_id, variant),
    FOREIGN KEY (tenant_id, document_id) REFERENCES documents (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, revision_id) REFERENCES revisions (tenant_id, id)
);
CREATE INDEX heads_search_idx ON heads USING gin (search);

-- Sections of every revision (bodies are derived from the blob on demand).
CREATE TABLE sections (
    tenant_id     uuid NOT NULL DEFAULT current_tenant(),
    revision_id   uuid NOT NULL,
    ordinal       integer NOT NULL,
    anchor        text NOT NULL,
    title         text NOT NULL,
    level         smallint NOT NULL,
    parent_anchor text,
    hash          bytea NOT NULL,
    PRIMARY KEY (tenant_id, revision_id, ordinal),
    FOREIGN KEY (tenant_id, revision_id) REFERENCES revisions (tenant_id, id) ON DELETE CASCADE
);

-- Outgoing links of each head. target_project NULL means the document's own project.
CREATE TABLE links (
    tenant_id      uuid NOT NULL DEFAULT current_tenant(),
    document_id    uuid NOT NULL,
    variant        text NOT NULL CHECK (variant IN ('human', 'ai')),
    ordinal        integer NOT NULL,
    target_project text,
    target_path    text,
    target_anchor  text,
    PRIMARY KEY (tenant_id, document_id, variant, ordinal),
    FOREIGN KEY (tenant_id, document_id) REFERENCES documents (tenant_id, id) ON DELETE CASCADE
);
CREATE INDEX links_target_idx ON links (tenant_id, target_path);

-- Semantic section hashes of both variants at the last reconciliation.
CREATE TABLE sync_bases (
    tenant_id   uuid NOT NULL DEFAULT current_tenant(),
    document_id uuid NOT NULL,
    anchor      text NOT NULL,
    human_hash  bytea,
    ai_hash     bytea,
    updated_at  timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, document_id, anchor),
    FOREIGN KEY (tenant_id, document_id) REFERENCES documents (tenant_id, id) ON DELETE CASCADE
);

-- Row-level security ------------------------------------------------------------

ALTER TABLE tenants ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON tenants TO solidate_app USING (id = current_tenant());

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'memberships', 'projects', 'blobs', 'documents', 'api_tokens',
        'revisions', 'heads', 'sections', 'links', 'sync_bases'
    ] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I TO solidate_app '
            'USING (tenant_id = current_tenant()) WITH CHECK (tenant_id = current_tenant())', t);
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON %I TO solidate_app', t);
    END LOOP;
END $$;

GRANT USAGE ON SCHEMA public TO solidate_app;
GRANT SELECT ON tenants TO solidate_app;
GRANT SELECT, INSERT, UPDATE ON users TO solidate_app;
GRANT SELECT, INSERT, UPDATE, DELETE ON sessions TO solidate_app;

-- Pre-tenant lookups ------------------------------------------------------------

CREATE FUNCTION tenant_by_slug(p_slug text)
    RETURNS TABLE (id uuid, slug text, name text, created_at timestamptz)
    LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public
    AS $$ SELECT id, slug, name, created_at FROM tenants WHERE slug = p_slug $$;

CREATE FUNCTION create_tenant(p_slug text, p_name text)
    RETURNS TABLE (id uuid, slug text, name text, created_at timestamptz)
    LANGUAGE sql VOLATILE SECURITY DEFINER SET search_path = public
    AS $$ INSERT INTO tenants (slug, name) VALUES (p_slug, p_name) RETURNING id, slug, name, created_at $$;

CREATE FUNCTION user_tenants(p_user uuid)
    RETURNS TABLE (id uuid, slug text, name text, created_at timestamptz, role text)
    LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public
    AS $$
        SELECT t.id, t.slug, t.name, t.created_at, m.role
        FROM memberships m JOIN tenants t ON t.id = m.tenant_id
        WHERE m.user_id = p_user
        ORDER BY t.slug
    $$;

CREATE FUNCTION api_token_by_prefix(p_prefix text)
    RETURNS TABLE (
        id uuid, tenant_id uuid, secret_hash bytea, scopes text[], project_id uuid,
        expires_at timestamptz, revoked_at timestamptz
    )
    LANGUAGE sql STABLE SECURITY DEFINER SET search_path = public
    AS $$
        SELECT id, tenant_id, secret_hash, scopes, project_id, expires_at, revoked_at
        FROM api_tokens WHERE prefix = p_prefix
    $$;

REVOKE ALL ON FUNCTION tenant_by_slug(text), create_tenant(text, text), user_tenants(uuid),
    api_token_by_prefix(text) FROM PUBLIC;
GRANT EXECUTE ON FUNCTION tenant_by_slug(text), create_tenant(text, text), user_tenants(uuid),
    api_token_by_prefix(text), current_tenant() TO solidate_app;
