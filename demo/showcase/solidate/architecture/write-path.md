# Write path

Every way of changing a document, whether the web editor, the REST API, MCP or an
import, ends in the same `put_doc` operation in the app layer. This page follows
one write from request to commit.

## Preconditions {#preconditions}

The caller states what it expects to replace. The web editor and MCP send the
content hash that was read. The REST API takes it from `If-Match`, accepts
`If-Match: *` to overwrite anything, and `If-None-Match: *` to insert a variant
that must not exist yet. A REST write with no precondition is rejected with
`428`, which makes blind overwrites a deliberate choice rather than an accident.

## Checks {#checks}

Before touching the document, Solidate checks the size limit and the caller's
write access to the project. If the path is not yet a document in this project
but is inherited from an ancestor, the write creates an *override* in this
project. An override that names an expected hash must match the inherited
version, so nobody overrides a page they have not read.

## Committing the revision {#commit}

The Markdown is analyzed into sections, anchors, links and hashes. The current
head of the variant is then locked with `SELECT … FOR UPDATE` and compared with
the expectation; a mismatch fails with `412` and the current hash. If the new
content is byte-identical to the head, nothing is written and the response says
`changed: false`. Otherwise the blob is stored, a revision is inserted with the
old head as its parent, and the head moves.

## After the write {#after}

- Any open proposal for the variant is dropped, because it was written against
  text that just changed.
- If sync is enabled, the section plan is recomputed. Anchors listed in
  `resolves` are reconciled. On the first write of a variant whose counterpart
  exists, sections present in both are reconciled automatically. Bases for
  sections that no longer exist on either side are pruned.
- An audit entry records the variant, revision, content hash and resolved
  anchors.

All of this happens in one tenant transaction, so a write is either entirely
visible, including its sync state and audit entry, or not at all.

## Response {#response}

The caller gets the new content hash, the revision id, whether anything changed,
and the list of sections in the document that still need sync. An agent can use
that list to decide what to translate next without another request.
