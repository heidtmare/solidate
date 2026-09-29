-- Undelete: restoring a deleted document.
--
-- deleted_by_user / deleted_by_token: who deleted the document; both NULL for the
-- CLI and internal jobs, and for documents deleted before this migration. Cleared
-- on undelete with deleted_at and deleted_xid.
--
-- restored_at / restored_xid: the last undelete, for the change feed. A document
-- deleted and restored within one feed window is reported as restored only.

ALTER TABLE documents ADD COLUMN deleted_by_user uuid REFERENCES users (id) ON DELETE SET NULL;
ALTER TABLE documents ADD COLUMN deleted_by_token uuid;
ALTER TABLE documents ADD COLUMN restored_at timestamptz;
ALTER TABLE documents ADD COLUMN restored_xid xid8;
CREATE INDEX documents_restored_xid_idx ON documents (tenant_id, restored_xid) WHERE restored_xid IS NOT NULL;
