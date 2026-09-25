# ADR 0001: Server-rendered UI

- status: accepted

## Context {#context}

- UI mostly read-only: rendered Markdown, outline, history, diffs.
- interactive: live preview (`POST /preview`), in-place sync item load (`/sync-item/...`).
- SPA cost: build pipeline, client state, duplicate rule enforcement.

## Decision {#decision}

- all pages server-rendered (Topcoat `#[page]`).
- htmx for partial updates only. No client-side framework.

## Consequences {#consequences}

- + fast loads; reading works without JS.
- + authz, rendering, validation implemented once (Rust).
- - rich client editing (collaborative cursors) needs a new design.
