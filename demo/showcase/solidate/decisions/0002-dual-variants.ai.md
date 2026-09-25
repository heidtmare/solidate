# ADR 0002: Human and AI variants

- status: accepted

## Context {#context}

- readers: people (context, reasoning) + coding agents (identifiers, constraints, no filler).
- single combined doc: serves neither. Two independent docs: drift.

## Decision {#decision}

- two variants per document (`human`, `ai`), same facts.
- pair by section anchor; sync state from semantic hashes.
- Solidate never generates/rewrites content. Translation by people/agents; agent translations of others' changes -> proposals -> human review.

## Consequences {#consequences}

- + audience-specific docs; drift visible per section.
- - every change needs translation (mitigated: agents).
- - differently worded headings need explicit `{#anchor}`.
