# Variants and sync

Each document has a human variant and an AI variant. This page explains how
Solidate decides whether they agree, what the sync states mean, and how to clear
them.

## How pairing works {#pairing}

Solidate splits both variants into sections and pairs them by anchor. For each
pair it stores a *sync base*: the semantic hash each side had the last time the
pair was agreed to be in sync. The semantic hash is computed over normalized
Markdown, so switching `*` bullets to `-`, `*emphasis*` to `_emphasis_`, or an
underlined heading to a `#` heading does not count as a change. Moving line
breaks within a paragraph does. See [[architecture/hashing]] for the details.

## Sync states {#states}

Comparing each side's current hash with the base gives one of four states.

| State | Meaning | What to do |
|---|---|---|
| In sync | Neither side changed since the base, or both sides are identical. | Nothing. |
| Human ahead | Only the human side changed. | Translate it into the AI variant. |
| AI ahead | Only the AI side changed. | Translate it into the human variant. |
| Conflict | Both sides changed. | Reconcile both into one truth. |

Adding or deleting a section is a change too: a section that exists only in the
human variant is *human ahead* until its AI counterpart is written or the human
section is removed.

## Where you see it {#where}

The document page shows a **Sync** button with the number of sections needing
attention, and the table of contents marks each stale section with a dot. The
project's **Sync** page lists every stale section across the project. Opening an
item shows both variants' current text, the text at the last sync, and a diff of
what changed since.

## Clearing a stale section {#clearing}

There are three ways to bring a pair back in sync.

1. **Edit the stale side and resolve.** Write the translation and list the
   section anchors it covers. The REST API and MCP take them as `resolves`; when
   you open the web editor from a sync item, it resolves that section on save.
2. **Accept a proposal.** An agent submits the translated variant; accepting it
   writes the revision and records the new base. See
   [[guide/agents-and-translation]].
3. **Mark in sync without editing.** When the change carries no meaning, such as
   a typo fix, resolve the section directly.

The first time a variant is written while its counterpart already exists,
sections present in both are marked in sync automatically.

## Turning sync off {#disable}

Some documents, like a changelog, only make sense in one variant. Sync can be
disabled per document from its sync page; it then no longer appears in the queue.
