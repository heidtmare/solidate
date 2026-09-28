-- Doc↔code drift (see solidate_core::sources).
--
-- source_snapshots / source_files: the repository files last reported for a
-- project, as path → hash. Hashes are opaque; clients report git blob object ids.
-- revision: the reporter's commit or other version label, if given.
--
-- source_verifications: per document section, the hashes of the files its source
-- patterns matched when someone last confirmed the section still describes them.

CREATE TABLE source_snapshots (
    tenant_id   uuid NOT NULL DEFAULT current_tenant(),
    project_id  uuid NOT NULL,
    revision    text,
    reported_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, project_id),
    FOREIGN KEY (tenant_id, project_id) REFERENCES projects (tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE source_files (
    tenant_id  uuid NOT NULL DEFAULT current_tenant(),
    project_id uuid NOT NULL,
    path       text NOT NULL,
    hash       text NOT NULL,
    PRIMARY KEY (tenant_id, project_id, path),
    FOREIGN KEY (tenant_id, project_id) REFERENCES projects (tenant_id, id) ON DELETE CASCADE
);

CREATE TABLE source_verifications (
    tenant_id       uuid NOT NULL DEFAULT current_tenant(),
    document_id     uuid NOT NULL,
    anchor          text NOT NULL,
    files           jsonb NOT NULL,
    revision        text,
    author_user_id  uuid REFERENCES users (id) ON DELETE SET NULL,
    author_token_id uuid,
    verified_at     timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (tenant_id, document_id, anchor),
    FOREIGN KEY (tenant_id, document_id) REFERENCES documents (tenant_id, id) ON DELETE CASCADE,
    FOREIGN KEY (tenant_id, author_token_id) REFERENCES api_tokens (tenant_id, id)
);

DO $$
DECLARE t text;
BEGIN
    FOREACH t IN ARRAY ARRAY['source_snapshots', 'source_files', 'source_verifications'] LOOP
        EXECUTE format('ALTER TABLE %I ENABLE ROW LEVEL SECURITY', t);
        EXECUTE format(
            'CREATE POLICY tenant_isolation ON %I TO solidate_app '
            'USING (tenant_id = current_tenant()) WITH CHECK (tenant_id = current_tenant())', t);
        EXECUTE format('GRANT SELECT, INSERT, UPDATE, DELETE ON %I TO solidate_app', t);
    END LOOP;
END $$;
