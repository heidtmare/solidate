# Writing style

These rules apply to every project that inherits from the handbook. A project can
override this page by writing its own `style/writing`.

## Voice {#voice}

Write for a capable colleague who has not seen the code. State what the system
does, then why. Prefer plain verbs and concrete nouns, and cut words that do not
carry information.

## Structure {#structure}

Start each document with one short paragraph that says what it covers. Use
second-level headings for the main topics and keep each section focused on one
idea, because sections are what Solidate pairs and tracks.

Give a heading an explicit anchor, such as `## Expiry {#expiry}`, whenever the
human and AI variants word it differently. Without one, the anchor comes from the
heading text and the two sections will not pair.

## Examples and references {#examples}

Show real commands, paths and payloads, copied from a working system. Link to the
document that owns a topic instead of repeating it, and use wiki links such as
`[[architecture/hashing]]` for cross-references so backlinks stay accurate.
