# Translation guide

Every document in projects that inherit from the handbook has a human variant and
an AI variant. They describe the same reality for different readers and are
translations of each other: every fact, decision and constraint in one must be in
the other, and neither adds facts the other lacks.

## Human variant

Narrative prose for engineers and operators. Explain context, reasoning and
trade-offs. Define a term the first time it appears, or link to
[[handbook:glossary]]. Use complete sentences and short paragraphs.

## AI variant

Dense reference for coding agents. Use bullet lists, `key: value` pairs and
tables. Give exact identifiers, paths, commands, HTTP methods, status codes,
limits and units. No narrative, no repetition, no marketing.

## Rules

- Keep the same sections in both variants so they pair up. When headings are
  worded differently, give both the same explicit anchor, e.g. `## Expiry {#expiry}`.
- Translate meaning, not wording: carry over every fact and change only the form.
- Do not resolve ambiguity by guessing. Keep it, and point it out in the proposal
  message.
- Keep links and `{{include}}` directives; they resolve per variant.
- Code blocks and commands are copied verbatim between variants.
