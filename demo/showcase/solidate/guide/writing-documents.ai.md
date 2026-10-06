# Writing documents

Format: GitHub-flavored Markdown. Covers Solidate-specific semantics. Style rules: [[style/writing]] (inherited from `handbook`).

## Paths {#paths}

- segments: `[A-Za-z0-9._-]+`; not `.` or `..`.
- max length: 512.
- normalization: trim `/`, strip trailing `.md`. `/design/auth.md` == `design/auth`.

## Sections and anchors {#sections}

- section: each heading -> next heading. Text before first heading: anchor `_preamble`.
- anchor: explicit `{#id}` suffix, else `slugify(heading text)`; duplicates -> `x`, `x-1`, ...
- anchors pair human/AI sections. Reworded heading without explicit anchor -> pair breaks.
- explicit anchor stripped from rendered text; used as HTML `id`.

```md
## How long a session lasts {#expiry}
```

## Links {#links}

- markdown link: relative to current doc dir; `/x` project-rooted; `scheme:` or `//` = external (untracked).
  - `[tokens](auth#tokens)` from `design/sessions` -> `design/auth#tokens`.
- wiki link: project-rooted; `[[path#anchor]]`, `[[project:path#anchor]]`, `[[path|label]]`.
- all internal links stored per head -> backlinks ("Linked from" panel).
- project link graph (`/t/{t}/p/{p}/graph`, REST `graph`, MCP `link_graph`): nodes = effective docs + missing targets + other-project targets; edges = links + includes from both variants. Unqualified links resolve against the viewing project, unqualified includes against the owning project.

## Includes {#includes}

```md
{{include handbook:glossary#core-terms}}
```

- syntax: `{{include [project:]path[#anchor]}}`, alone in a paragraph.
- anchor: selects section + subsections.
- max depth: 8.
- cycle / missing / depth exceeded: rendered notice (`> **Missing include:** ...`), no error.
- resolved hash = Merkle(content hash, dependency hashes) -> upstream change changes it.

## Source bindings {#sources}

- `<!-- sources: p1, p2 -->` on its own lines binds the enclosing section to repo files; not rendered; excluded from semantic hashes. See [[guide/code-drift]].

## Other Markdown features {#markdown}

- enabled: tables, task lists, strikethrough, autolinks, footnotes, wiki links.
- raw HTML: never rendered.

- [x] Tables and task lists
- [x] Footnotes[^1]
- [ ] Raw HTML

[^1]: Footnotes render at the end of the page.
