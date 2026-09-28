-- Change feed (see solidate_app::changes).
--
-- xid / deleted_xid: id of the transaction that wrote the row or deleted the
-- document. A feed window is a range of transaction ids whose upper bound is
-- pg_snapshot_xmin(pg_current_snapshot()) at read time: every transaction below it
-- has finished, so a transaction that commits late still lands in a later window.
-- Existing rows get the migration's transaction id.

ALTER TABLE documents ADD COLUMN xid xid8 NOT NULL DEFAULT pg_current_xact_id();
ALTER TABLE documents ADD COLUMN deleted_xid xid8;
UPDATE documents SET deleted_xid = pg_current_xact_id() WHERE deleted_at IS NOT NULL;
CREATE INDEX documents_deleted_xid_idx ON documents (tenant_id, deleted_xid) WHERE deleted_xid IS NOT NULL;

ALTER TABLE revisions ADD COLUMN xid xid8 NOT NULL DEFAULT pg_current_xact_id();
CREATE INDEX revisions_xid_idx ON revisions (tenant_id, xid);
