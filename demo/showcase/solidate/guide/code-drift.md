# Keeping docs in step with code

Human/AI sync catches one kind of drift: the two variants of a document saying
different things. Source bindings catch the other kind: the code changes and the
documentation describing it no longer matches. A section declares the repository
files it describes, a client reports the repository's file hashes, and Solidate
lists every section whose files changed since someone last confirmed it was still
accurate.

## Declaring sources {#bindings}

<!-- sources: crates/solidate-core/src/sources.rs, crates/solidate-core/src/markdown.rs -->

Put an HTML comment on its own lines inside the section:

```md
## Token validation {#tokens}

<!-- sources: crates/solidate-app/src/auth.rs, crates/solidate-db/migrations/ -->
```

Patterns are relative to the repository root and separated by commas or line
breaks. `*` and `?` match within one path segment, `**` matches any number of
segments, and a trailing `/` covers everything below a directory. The comment
belongs to the section it appears in; before the first heading it applies to the
preamble. Either variant can declare bindings, and a document's bindings are the
union of both.

Binding comments are not rendered and are left out of semantic hashes, so adding
one never marks the other variant stale. Invalid patterns, such as absolute paths
or `..`, are rejected when the document is written.

## Reporting the repository {#reporting}

<!-- sources: crates/solidate-app/src/sources.rs, crates/solidate-cli/src/main.rs -->

Solidate never reads the repository itself. A client reports `path → hash` for the
project, normally CI on every push to the main branch. Hashes are opaque to
Solidate; every reporter should send git blob ids, as printed by
`git ls-files -s`, so that reports from different tools agree.

```sh
solidate sources report acme docs --dir . # complete tree at HEAD
```

Over HTTP, `POST /api/v1/projects/{p}/sources` takes the same data, either as the
complete tree (`replace: true`) or as changed and removed files. A report carries
an optional revision, usually the commit id.

## The drift queue {#queue}

<!-- sources: crates/solidate-app/src/sources.rs -->

A bound section is in one of three states:

- **unverified**: nobody has confirmed it against its sources yet;
- **changed**: files it matches changed, appeared or disappeared since it was last
  verified;
- **fresh**: the files it matches are exactly those it was verified against.

The drift queue lists every bound section that is not fresh, plus sections with
patterns that match no reported file, which usually means a file was renamed or
deleted. Each entry names the changed, added and removed paths and the revision
the section was last verified at, so `git diff <revision> -- <paths>` shows
exactly what the section needs to catch up with.

## Working through drift {#workflow}

For each entry, either update the section so it matches the code, or confirm it is
still accurate. Either way, finish by verifying it, which records the hashes of
the files it matches now. An edit to one variant then shows up in the sync queue
as usual, so the translation follows.

An agent that has just changed code can skip the queue: `affected_sections` takes
the changed paths and returns the sections that describe them, even before a new
report arrives. Verification accepts the revision the agent checked against and
fails if a newer report has arrived since.

Removing a section's bindings from both variants discards its verification.

This page binds its own sections to the code that implements them.
