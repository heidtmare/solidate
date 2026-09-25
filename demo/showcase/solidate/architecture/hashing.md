# Hashing

Hashes are how Solidate knows what changed, at every scale from one section to a
whole project. All of them are BLAKE3, and they come in two kinds that answer
different questions.

## Content hashes {#content}

A content hash covers the exact bytes someone wrote. Change a single space and
the hash changes. It identifies the stored blob, so identical content is stored
once, and it is the HTTP `ETag` of a variant, which makes it the token for
optimistic concurrency: a writer sends the hash it read and the write succeeds
only if that is still the current one.

## Semantic hashes {#semantic}

A semantic hash covers the *normalized* Markdown of a section: the document is
parsed and re-emitted as canonical CommonMark before hashing. Two sections that
differ only in Markdown markers, such as a `Setext` versus an `ATX` heading, `*`
versus `_` for emphasis, or `*` versus `-` for bullets, have the same semantic
hash. Line breaks inside a paragraph are kept by normalization, so rewrapping text
does count as a change.

Sync compares semantic hashes. That is the point of having two kinds: reformatting
the human variant should not ask anyone to re-translate the AI variant.

## Merkle roots {#merkle}

Solidate rolls many hashes into one with a simple Merkle construction: entries are
sorted by key, and each is fed to the hasher as `key`, a zero byte, the hex hash
and a newline. The result does not depend on insertion order.

The project root hash covers every effective document, including inherited
ones, by `path@variant`. A client that wants to know whether anything in a project
changed makes one request for the root hash, and the server answers
`304 Not Modified` if it has not.

## Resolved hashes {#resolved}

A document that includes other documents has a *resolved hash*: its own content
hash combined with the hash of every piece it includes, in order. Because an
upstream edit changes the resolved hash, caches of the expanded page stay honest.
The readme of this project, which includes the handbook glossary, is an example.
