# Writing documents

Solidate documents are GitHub-flavored Markdown. This page covers the parts of
the syntax that Solidate gives extra meaning to: paths, sections and anchors,
links, and includes. The house style lives in [[style/writing]], inherited from
the handbook.

## Paths {#paths}

A document is identified by a path inside its project, such as `design/auth`.
Segments may contain ASCII letters, digits, `-`, `_` and `.`, and paths are at
most 512 characters. A leading or trailing slash and a trailing `.md` are
dropped, so `/design/auth.md` and `design/auth` name the same document.

## Sections and anchors {#sections}

Every heading starts a section that runs to the next heading. Text before the
first heading forms a preamble section of its own. Each section gets an anchor:
the explicit `{#id}` written after the heading if there is one, otherwise a slug
of the heading text. Repeated headings get numbered anchors (`tokens`,
`tokens-1`).

Anchors matter because they are how a human section finds its AI counterpart.
If you reword a heading in one variant only, its derived anchor changes and the
pair breaks. An explicit anchor prevents that:

```md
## How long a session lasts {#expiry}
```

The `{#expiry}` suffix is removed from the rendered heading and becomes the
heading's HTML `id`.

## Links {#links}

Solidate understands two link forms and records both for backlinks.

- Markdown links resolve relative to the current document: `[tokens](auth#tokens)`
  from `design/sessions` points at `design/auth#tokens`. A leading `/` resolves
  from the project root. Links with a scheme, such as `https:`, are external.
- Wiki links are always project-rooted and can cross projects:
  `[[architecture/hashing]]`, `[[handbook:glossary#core-terms]]`.

Every document page lists the documents that link to it under **Linked from**.

## Includes {#includes}

An include directive, written alone on its own line, transcludes another
document or one section of it, including its subsections:

```md
{{include handbook:glossary#core-terms}}
```

Includes expand recursively up to eight levels. Cycles, missing targets and
excessive depth render as a visible notice instead of failing the page. The
hashes of included content are folded into the including document's *resolved
hash*, so a change upstream is visible to anyone caching the expanded page. The
readme of this project includes the glossary this way.

## Other Markdown features {#markdown}

Tables, task lists, strikethrough, autolinks and footnotes are supported. Raw
HTML is never rendered, which keeps pages safe to display regardless of who wrote
them.

- [x] Tables and task lists
- [x] Footnotes[^1]
- [ ] Raw HTML

[^1]: Footnotes render at the end of the page.
