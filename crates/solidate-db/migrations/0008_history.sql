-- History: restores and past sync pairings.
--
-- revisions.restored_from: the revision whose content a restore wrote back. A
-- restore is a new revision; heads never move backwards.
--
-- sync_base_log: every sync base ever recorded, append-only. `sync_bases` holds
-- only the current base per anchor and drops it when the section disappears; the
-- log keeps which section hashes of the two variants were once accepted as in
-- sync, so a restore can bring back the matching counterpart and its base.
-- Existing bases are copied in as the log's first entries.

ALTER TABLE revisions ADD COLUMN restored_from uuid;
ALTER TABLE revisions ADD CONSTRAINT revisions_restored_from_fkey
    FOREIGN KEY (tenant_id, restored_from) REFERENCES revisions (tenant_id, id);

CREATE TABLE sync_base_log (
    tenant_id   uuid NOT NULL DEFAULT current_tenant(),
    document_id uuid NOT NULL,
    anchor      text NOT NULL,
    human_hash  bytea,
    ai_hash     bytea,
    xid         xid8 NOT NULL DEFAULT pg_current_xact_id(),
    created_at  timestamptz NOT NULL DEFAULT now(),
    FOREIGN KEY (tenant_id, document_id) REFERENCES documents (tenant_id, id) ON DELETE CASCADE,
    CHECK (human_hash IS NOT NULL OR ai_hash IS NOT NULL)
);
CREATE INDEX sync_base_log_anchor_idx ON sync_base_log (tenant_id, document_id, anchor, created_at DESC);

INSERT INTO sync_base_log (tenant_id, document_id, anchor, human_hash, ai_hash, created_at)
SELECT tenant_id, document_id, anchor, human_hash, ai_hash, updated_at
FROM sync_bases
WHERE human_hash IS NOT NULL OR ai_hash IS NOT NULL;

ALTER TABLE sync_base_log ENABLE ROW LEVEL SECURITY;
CREATE POLICY tenant_isolation ON sync_base_log TO solidate_app
    USING (tenant_id = current_tenant()) WITH CHECK (tenant_id = current_tenant());
GRANT SELECT, INSERT ON sync_base_log TO solidate_app;
