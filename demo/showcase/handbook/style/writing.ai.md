# Writing style

Applies to all projects inheriting `handbook`. Override by defining `style/writing` in the child project.

## Voice {#voice}

- audience: capable engineer, unfamiliar with the code.
- order: what the system does, then why.
- plain verbs, concrete nouns; remove words carrying no information.

## Structure {#structure}

- open with one paragraph stating scope.
- `##` per main topic; one idea per section (sections are the sync unit).
- headings worded differently between variants MUST carry the same explicit anchor: `## Expiry {#expiry}`. Otherwise anchors derive from text and do not pair.

## Examples and references {#examples}

- use real commands, paths, payloads from a working system.
- link to the owning document; do not duplicate content.
- cross-references: wiki links, e.g. `[[architecture/hashing]]` (feeds backlinks).
