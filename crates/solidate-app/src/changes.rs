//! Change feed: what changed in a project's effective documents since a cursor or
//! a point in time, as net section changes per document variant.
//!
//! A cursor is a transaction-id bound (see migration `0007_changes.sql`). Each
//! response's `cursor` starts the next window, so successive windows cover every
//! committed change exactly once.

use std::collections::HashMap;

use solidate_core::{Hash, Variant};
use solidate_db::{ChangeRevision, ChangeWindow, DocumentId, ProjectId, RevisionId};
use time::OffsetDateTime;
use time::format_description::well_known::Rfc3339;

use crate::App;
use crate::ctx::{Access, Ctx};
use crate::error::{Result, invalid};
use crate::projects::project_by_slug;

#[derive(Debug, Clone, serde::Serialize)]
pub struct Changes {
    /// Pass as `since` to continue after this response.
    pub cursor: String,
    /// Ordered by path.
    pub documents: Vec<DocChange>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DocChange {
    pub path: String,
    pub title: Option<String>,
    /// Slug of the project that owns the document.
    pub owner: String,
    pub inherited: bool,
    /// The document was created in the window. An override of an inherited
    /// document counts as created.
    pub created: bool,
    /// The document was deleted in the window; `variants` is empty.
    pub deleted: bool,
    pub variants: Vec<VariantChange>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct VariantChange {
    pub variant: Variant,
    /// Number of revisions in the window.
    pub revisions: usize,
    /// Content hash after the window's last revision.
    pub content_hash: Hash,
    /// Section anchors, compared by semantic hash between the revision before the
    /// window and the last revision in it. All empty for formatting-only changes.
    pub added: Vec<String>,
    pub changed: Vec<String>,
    pub removed: Vec<String>,
    pub authors: Vec<ChangeAuthor>,
    /// Revision messages in write order.
    pub messages: Vec<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChangeAuthor {
    /// `user`, `token` or `system`.
    pub kind: String,
    pub name: Option<String>,
}

enum Since {
    Cursor(u64),
    Time(OffsetDateTime),
}

fn parse_since(s: &str) -> Result<Since> {
    let s = s.trim();
    if let Ok(x) = s.parse::<u64>() {
        return Ok(Since::Cursor(x));
    }
    OffsetDateTime::parse(s, &Rfc3339)
        .map(Since::Time)
        .map_err(|_| invalid("since must be a cursor from a previous response or an RFC 3339 timestamp"))
}

impl App {
    /// Changes to the effective documents of `project` since `since`: a cursor from
    /// a previous response or an RFC 3339 timestamp. Without `since`, returns only a
    /// cursor for the current point. Changes appear once every transaction that
    /// started before them has finished.
    pub async fn changes(&self, ctx: &Ctx, project: &str, since: Option<&str>) -> Result<Changes> {
        let since = since.map(parse_since).transpose()?;
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        // Read committed: later statements see every transaction below the horizon.
        let hi = tx.change_horizon().await?;
        let Some(since) = since else {
            return Ok(Changes {
                cursor: hi,
                documents: Vec::new(),
            });
        };
        let w = match since {
            // Never hand back a cursor below the one given.
            Since::Cursor(c) if hi.parse::<u64>().is_ok_and(|h| c > h) => {
                return Ok(Changes {
                    cursor: c.to_string(),
                    documents: Vec::new(),
                });
            }
            Since::Cursor(c) => ChangeWindow {
                hi: hi.clone(),
                lo: Some(c.to_string()),
                since: None,
            },
            Since::Time(t) => ChangeWindow {
                hi: hi.clone(),
                lo: None,
                since: Some(t),
            },
        };

        let chain = tx.project_chain(p.id).await?;
        let ids: Vec<ProjectId> = chain.iter().map(|p| p.id).collect();
        let revisions = tx.change_revisions(&ids, &w).await?;
        let mut groups: Vec<((DocumentId, Variant), Vec<ChangeRevision>)> = Vec::new();
        for r in revisions {
            match groups.last_mut() {
                Some((k, g)) if *k == (r.document_id, r.variant) => g.push(r),
                _ => groups.push(((r.document_id, r.variant), vec![r])),
            }
        }
        let mut doc_ids: Vec<DocumentId> = groups.iter().map(|((d, _), _)| *d).collect();
        doc_ids.dedup();
        let documents = tx.changed_documents(&ids, &doc_ids, &w).await?;

        let bounds: Vec<RevisionId> = groups
            .iter()
            .flat_map(|(_, g)| g[0].parent_id.into_iter().chain([g[g.len() - 1].id]))
            .collect();
        let mut sections: HashMap<RevisionId, Vec<(String, Hash)>> = HashMap::new();
        for (rev, anchor, hash) in tx.section_hashes(&bounds).await? {
            sections.entry(rev).or_default().push((anchor, hash));
        }

        let mut out = Vec::new();
        for d in documents {
            let depth = chain.iter().position(|c| c.id == d.project_id).unwrap_or(0);
            let mut shadowed = false;
            for nearer in &chain[..depth] {
                if tx
                    .document_by_path(nearer.id, &d.path.parse().map_err(invalid)?)
                    .await?
                    .is_some()
                {
                    shadowed = true;
                    break;
                }
            }
            if shadowed {
                continue;
            }
            let variants = if d.deleted {
                Vec::new()
            } else {
                groups
                    .iter()
                    .filter(|((id, _), _)| *id == d.id)
                    .map(|((_, variant), g)| variant_change(*variant, g, &sections))
                    .collect()
            };
            out.push(DocChange {
                path: d.path,
                title: d.title,
                owner: chain[depth].slug.clone(),
                inherited: depth > 0,
                created: d.created,
                deleted: d.deleted,
                variants,
            });
        }
        Ok(Changes {
            cursor: hi,
            documents: out,
        })
    }
}

fn variant_change(
    variant: Variant,
    g: &[ChangeRevision],
    sections: &HashMap<RevisionId, Vec<(String, Hash)>>,
) -> VariantChange {
    let empty = Vec::new();
    let last = &g[g.len() - 1];
    let before = g[0].parent_id.and_then(|p| sections.get(&p)).unwrap_or(&empty);
    let after = sections.get(&last.id).unwrap_or(&empty);
    let old: HashMap<&str, Hash> = before.iter().map(|(a, h)| (a.as_str(), *h)).collect();
    let new: HashMap<&str, Hash> = after.iter().map(|(a, h)| (a.as_str(), *h)).collect();
    let mut added = Vec::new();
    let mut changed = Vec::new();
    for (a, h) in after {
        match old.get(a.as_str()) {
            None => added.push(a.clone()),
            Some(o) if o != h => changed.push(a.clone()),
            Some(_) => {}
        }
    }
    let removed = before
        .iter()
        .filter(|(a, _)| !new.contains_key(a.as_str()))
        .map(|(a, _)| a.clone())
        .collect();

    let mut authors: Vec<ChangeAuthor> = Vec::new();
    for r in g {
        let a = ChangeAuthor {
            kind: r.author_kind.clone(),
            name: r.author_name.clone(),
        };
        if !authors.contains(&a) {
            authors.push(a);
        }
    }
    VariantChange {
        variant,
        revisions: g.len(),
        content_hash: last.content_hash,
        added,
        changed,
        removed,
        authors,
        messages: g.iter().filter_map(|r| r.message.clone()).collect(),
        updated_at: last.created_at,
    }
}
