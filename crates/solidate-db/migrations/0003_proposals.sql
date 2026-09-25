-- Translation proposals: a full replacement of one variant, submitted (typically by
-- an agent) to carry changes over from the other variant. Accepting one writes it as
-- a revision and marks `resolves` in sync. At most one proposal per document variant;
-- a new submission replaces the previous one.
--
-- base_hash: head content hash of the target variant the proposal replaces (NULL:
-- the variant did not exist). source_hash: head content hash of the other variant
-- the proposal translates. A proposal is outdated once either head moves.

CREATE TABLE proposals (
    id              uuid NOT NULL,
    tenant_id       uuid NOT NULL DEFAULT current_tenant() REFERENCES tenants (id) ON DELETE CASCADE,
    document_id     uuid NOT NULL,
    variant         text NOT NULL CHECK (variant IN ('human', 'ai')),
    base_hash       bytea,
    source_hash     bytea,
    content         text NOT NULL,
    resolves        text[] NOT NULL DEFAULT '{}',
    message         text,
    author_user_id  uuid REFERENCES users (id) ON DELETE SET NULL,
    author_token_id uuid,
    created_at      timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, id),
    UNIQUE (id),
    UNIQUE (tenant_id, document_id, variant),
    FOREIGN KEY (tenant_id, document_id) REFERENCES documents (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, author_token_id) REFERENCES api_tokens (tenant_id, id)
);

ALTER TABLE proposals ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON proposals TO solidate_app
    USING (tenant_id = current_tenant()) WITH CHECK (tenant_id = current_tenant());
GRANT SELECT, INSERT, UPDATE, DELETE ON proposals TO solidate_app;
