# Translation guide

Scope: every document in projects inheriting `handbook`. Two variants (`human`, `ai`), same facts; neither adds facts the other lacks.

## Human variant

- audience: engineers, operators.
- form: narrative prose; context, reasoning, trade-offs; complete sentences; short paragraphs.
- terms: define on first use or link [[handbook:glossary]].

## AI variant

- audience: coding agents.
- form: bullets, `key: value`, tables.
- include exact identifiers, paths, commands, HTTP methods, status codes, limits, units.
- exclude narrative, repetition, marketing.

## Rules

- same sections in both variants; differently worded headings share an explicit anchor (`## Expiry {#expiry}`).
- translate meaning, not wording; carry every fact.
- ambiguity: keep it; flag it in the proposal `message`.
- keep links and `{{include}}` directives (resolve per variant).
- copy code blocks and commands verbatim.
