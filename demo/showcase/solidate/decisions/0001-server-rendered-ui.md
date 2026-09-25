# ADR 0001: Server-rendered UI

**Status:** Accepted

## Context {#context}

Solidate's interface is mostly reading: rendered Markdown, a table of contents,
history and diffs. The interactive parts are small, such as a live preview while
editing and loading a sync item in place. A single-page application would add a
build pipeline, a client-side state layer and a second place to enforce rules.

## Decision {#decision}

Render every page on the server with Topcoat, and use htmx for the few
interactions that benefit from partial updates. No client-side framework.

## Consequences {#consequences}

Pages are fast to load and work without JavaScript for reading. Authorization,
rendering and validation exist once, in Rust. The trade-off is that rich
client-side editing, such as collaborative cursors, would need a new design
rather than a component.
