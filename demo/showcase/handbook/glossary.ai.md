# Glossary

Shared term definitions. Include with `{{include handbook:glossary#core-terms}}`; do not redefine.

## Core terms {#core-terms}

- tenant: isolated organization; all tenant rows carry `tenant_id`; Postgres RLS enforces isolation.
- project: document collection in a tenant; optional `parent`; child inherits ancestor docs by path; own doc at same path overrides.
- document: Markdown page at a path (e.g. `architecture/hashing`); two variants.
- variant: `human` (narrative) | `ai` (dense, structured). Same facts.
- section: heading to next heading. Unit of sync tracking.
- anchor: section id; explicit `{#id}` else slug of heading text; pairs human/AI sections.

## Sync terms {#sync-terms}

- sync base: per-anchor semantic hashes of both sides at last reconciliation.
- stale side: variant needing translation.
- proposal: full replacement of one variant, typically from an agent; reviewed (accept | reject).
- translation guide: doc at `_meta/translation`; how variants differ in this project.
