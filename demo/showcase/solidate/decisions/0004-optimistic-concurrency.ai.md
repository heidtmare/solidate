# ADR 0004: Optimistic concurrency with content hashes

- status: accepted

## Context {#context}

- concurrent editors: people + agents, unaware of each other.
- pessimistic locks: agents blocked by idle editors. last-write-wins: silent loss.

## Decision {#decision}

- every write carries expected content hash.
- server: `SELECT ... FOR UPDATE` on head for write duration; mismatch -> 412 + current hash.
- HTTP: `ETag` / `If-Match`; no precondition -> 428.

## Consequences {#consequences}

- + no unseen overwrite; conflict error includes retry info.
- + web editor preserves user text on conflict (409 page).
- - read-before-write required.
