# Variants and sync

Each document has a human variant and an AI variant. This page explains how
Solidate decides whether they agree, what the sync states mean, and how to clear
them.

## How pairing works {#pairing}

Solidate splits both variants into sections and pairs them by anchor. For each
pair it stores a *sync base*: the sync hash each side had the last time the
pair was agreed to be in sync. The sync hash is computed over normalized
Markdown, so switching `*` bullets to `-`, `*emphasis*` to `_emphasis_`, or an
underlined heading to a `#` heading does not count as a change. Moving line
breaks within a paragraph does. The contents of diagrams are left out too; see
[[#diagrams]]. See [[architecture/hashing]] for the details.

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

## Documents with one variant {#single}

A document with only one variant has nothing to keep in sync, so none of its
sections are stale. Writing a human variant alone creates no work for anyone.
Sync starts when the second variant is first written, by an agent or by hand.
Agents find these documents in the *untranslated* list
(`GET /api/v1/projects/{p}/untranslated`, MCP `get_untranslated`,
`solidate untranslated`).

## Diagrams {#diagrams}

A diagram fence (a ` ```mermaid ` block) is shared by both variants rather than
translated. Sync ignores what is inside it, so editing a diagram never marks the
other variant stale. When the human variant's diagram changes and the same
section of the AI variant holds an identical copy of the old diagram, Solidate
updates that copy in the same write. Adding or removing a diagram still counts
as a change, so the other variant picks it up through the usual queue.

After saving, the document page says which diagram edits were applied to the AI
variant and which were not because its copy differs. Diagrams whose contents
differ between the variants are marked with a square in the table of contents
and listed under **Diagrams that differ** on the Sync pages, drawn side by side.
Choose **Use in AI variant** or **Use in human variant** to make both match.

A diagram that fails to render shows the error message and its numbered source,
with the failing line highlighted. In the editor preview the last version that
rendered stays below the error, and a note next to **Save** lists the document
line of each error; selecting one selects that line in the editor.

## Where you see it {#where}

The document page shows a **Sync** button with the number of sections a person
needs to act on: the AI variant changed, or both did. The table of contents
marks each of them with a dot, and sections waiting on agents (the human
variant changed) with a hollow dot. The project's **Sync** page lists every
stale section across the project in two groups: *Needs a person* and *Waiting
on agents*. Opening an
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
