# ADR 0002: Human and AI variants

**Status:** Accepted

## Context {#context}

Design documentation is increasingly read by coding agents as well as people.
The two audiences want different things: people need context and reasoning,
agents need exact identifiers and constraints with no filler. One document that
tries to serve both serves neither well, and two independent documents drift.

## Decision {#decision}

Store every document as two variants of the same truth, pair them section by
section through anchors, and track each pair's sync state from semantic hashes.
Solidate never generates or rewrites content itself; people and agents do the
translation, and agents' translations go through human review as proposals.

## Consequences {#consequences}

Each audience reads a document written for it, and drift is visible at section
granularity instead of silently accumulating. Writing costs more, since every
change needs a translation, which is why agents do most of that work. Headings
that differ between variants need explicit anchors to pair.
