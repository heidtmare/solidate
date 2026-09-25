# ADR 0004: Optimistic concurrency with content hashes

**Status:** Accepted

## Context {#context}

People and agents edit the same documents, often at the same time and without
seeing each other. Locks would block agents on people who wandered off with an
editor open, and last-write-wins would silently discard work.

## Decision {#decision}

Every write names the content hash it expects to replace. The server locks the
head row only for the duration of the write, compares, and fails with the current
hash on mismatch. Over HTTP this is standard `ETag` and `If-Match`, and a write
without a precondition is refused.

## Consequences {#consequences}

No one can overwrite a change they have not seen, and conflicts surface as a
clear error with the information needed to retry. The web editor keeps the
user's text on a conflict so nothing is lost. Clients must read before they
write, which agents do anyway.
