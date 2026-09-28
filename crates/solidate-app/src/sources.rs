//! Doc↔code drift: source reports, the drift queue, reverse lookup and
//! verification. Binding and state rules are in [`solidate_core::sources`].

use std::collections::HashMap;

use serde_json::json;
use solidate_core::markdown::{analyze, source_bindings};
use solidate_core::sources::{
    Delta, DriftState, Files, SourceBinding, SourcePattern, classify, matched, merge_bindings, missing, parse_patterns,
    validate_file,
};
use solidate_core::{DocPath, Hash, Variant};
use solidate_db::{DocumentId, SourceSnapshot, TenantTx};
use time::OffsetDateTime;

use crate::App;
use crate::audit::record;
use crate::ctx::{Access, Ctx};
use crate::error::{Result, invalid};
use crate::projects::project_by_slug;
use crate::sync::own_doc;

/// Maximum number of files in one report.
pub const MAX_REPORT_FILES: usize = 200_000;
const MAX_REVISION_LEN: usize = 200;

/// Repository files reported by a client.
pub struct SourceReport<'a> {
    /// Commit id or other version label of the reported tree.
    pub revision: Option<&'a str>,
    /// Path → content hash (git blob object id).
    pub files: &'a Files,
    /// Paths to delete. Only without `replace`.
    pub removed: &'a [String],
    /// `files` is the complete tree: every other reported file is removed.
    pub replace: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SnapshotSummary {
    pub revision: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub reported_at: OffsetDateTime,
    /// Number of files reported for the project.
    pub files: i64,
}

/// Drift status of one bound section.
#[derive(Debug, Clone, serde::Serialize)]
pub struct DriftEntry {
    pub path: String,
    pub document_title: Option<String>,
    pub anchor: String,
    pub section_title: String,
    pub state: DriftState,
    pub patterns: Vec<String>,
    /// Patterns that match no reported file.
    pub missing: Vec<String>,
    /// Changes since the last verification.
    #[serde(flatten)]
    pub delta: Delta,
    /// Revision of the report the section was last verified against.
    pub verified_revision: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub verified_at: Option<OffsetDateTime>,
}

impl DriftEntry {
    pub fn needs_attention(&self) -> bool {
        self.state != DriftState::Fresh || !self.missing.is_empty()
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DriftQueue {
    /// Revision of the current report; `None` also when nothing was reported.
    pub revision: Option<String>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub reported_at: Option<OffsetDateTime>,
    pub entries: Vec<DriftEntry>,
}

/// A bound section whose patterns match some of the queried paths.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AffectedSection {
    pub path: String,
    pub document_title: Option<String>,
    pub anchor: String,
    pub section_title: String,
    /// The queried paths the section's patterns match.
    pub paths: Vec<String>,
}

/// A bound section with its content, for reading before changing the files it
/// describes.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SectionContext {
    pub path: String,
    pub document_title: Option<String>,
    pub anchor: String,
    pub section_title: String,
    /// The variant `content` comes from.
    pub variant: Variant,
    /// Section Markdown, starting with its heading line (without subsections).
    pub content: String,
    /// Semantic hash of the section (`section_hash` for a section write).
    pub hash: Hash,
    /// Content hash of the variant (`base_hash` for a document write).
    pub content_hash: Hash,
    /// The queried paths the section's patterns match.
    pub paths: Vec<String>,
    /// Internal link targets in the section (`project:path#anchor`), to follow for
    /// related context such as decision records.
    pub links: Vec<String>,
}

struct DocBindings {
    bindings: Vec<SourceBinding>,
    titles: HashMap<String, String>,
}

/// Bindings of both variants' heads, human first, with section titles by anchor.
async fn doc_bindings(tx: &mut TenantTx, document: DocumentId) -> Result<DocBindings> {
    let mut all = Vec::new();
    let mut titles = HashMap::new();
    for variant in Variant::ALL {
        let Some(head) = tx.head(document, variant).await? else {
            continue;
        };
        all.extend(source_bindings(&head.content));
        for s in tx.sections(head.revision_id).await? {
            titles.entry(s.anchor).or_insert(s.title);
        }
    }
    Ok(DocBindings {
        bindings: merge_bindings(all),
        titles,
    })
}

async fn doc_entries(
    tx: &mut TenantTx,
    document: DocumentId,
    path: &str,
    title: Option<&str>,
    files: &Files,
) -> Result<Vec<DriftEntry>> {
    let DocBindings { bindings, titles } = doc_bindings(tx, document).await?;
    if bindings.is_empty() {
        return Ok(Vec::new());
    }
    let mut records = tx.source_verifications(document).await?;
    Ok(bindings
        .into_iter()
        .map(|b| {
            let current = matched(&parse_patterns(&b.patterns), files);
            let record = records.remove(&b.anchor);
            let (state, delta) = classify(record.as_ref().map(|r| &r.files), &current);
            DriftEntry {
                path: path.to_owned(),
                document_title: title.map(str::to_owned),
                section_title: titles.get(&b.anchor).cloned().unwrap_or_default(),
                state,
                missing: missing(&b.patterns, files),
                delta,
                verified_revision: record.as_ref().and_then(|r| r.revision.clone()),
                verified_at: record.map(|r| r.verified_at),
                anchor: b.anchor,
                patterns: b.patterns,
            }
        })
        .collect())
}

/// Rejects writes whose source directives contain invalid patterns.
pub(crate) fn validate_bindings(bindings: &[SourceBinding]) -> Result<()> {
    for b in bindings {
        for p in &b.patterns {
            SourcePattern::parse(p).map_err(|e| invalid(format!("section #{}: {e}", b.anchor)))?;
        }
    }
    Ok(())
}

/// Drops verification records of anchors no longer bound in either variant.
/// `written` are the bindings of the variant just written.
pub(crate) async fn prune_verifications(
    tx: &mut TenantTx,
    document: DocumentId,
    variant: Variant,
    written: &[SourceBinding],
) -> Result<()> {
    let mut keep: Vec<String> = written.iter().map(|b| b.anchor.clone()).collect();
    if let Some(other) = tx.head(document, variant.other()).await? {
        keep.extend(source_bindings(&other.content).into_iter().map(|b| b.anchor));
    }
    tx.prune_source_verifications(document, &keep).await?;
    Ok(())
}

fn files_of(snapshot: &Option<SourceSnapshot>) -> Files {
    snapshot.as_ref().map(|s| s.files.clone()).unwrap_or_default()
}

impl App {
    /// Records the repository files of `project` as reported by a client.
    pub async fn report_sources(&self, ctx: &Ctx, project: &str, r: SourceReport<'_>) -> Result<SnapshotSummary> {
        if r.files.len() > MAX_REPORT_FILES || r.removed.len() > MAX_REPORT_FILES {
            return Err(invalid(format!("a report holds at most {MAX_REPORT_FILES} files")));
        }
        if r.replace && !r.removed.is_empty() {
            return Err(invalid("`removed` cannot be combined with `replace`"));
        }
        if let Some(rev) = r.revision
            && (rev.is_empty() || rev.len() > MAX_REVISION_LEN || rev.chars().any(char::is_control))
        {
            return Err(invalid(format!(
                "revision must be 1 to {MAX_REVISION_LEN} characters without control characters"
            )));
        }
        for (path, hash) in r.files {
            validate_file(path, hash).map_err(invalid)?;
        }
        for path in r.removed {
            validate_file(path, "-").map_err(invalid)?;
        }

        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Write, Some(&p))?;
        let files = tx
            .put_source_files(p.id, r.revision, r.files, r.removed, r.replace)
            .await?;
        let detail = json!({
            "revision": r.revision,
            "files": r.files.len(),
            "removed": r.removed.len(),
            "replace": r.replace,
        });
        record(&mut tx, ctx, "sources.report", Some(p.id), None, detail).await?;
        let snapshot = tx.source_snapshot(p.id).await?.expect("just written");
        tx.commit().await?;
        Ok(SnapshotSummary {
            revision: snapshot.revision,
            reported_at: snapshot.reported_at,
            files,
        })
    }

    /// Bound sections of `project`'s own documents that are unverified, whose
    /// sources changed since verification, or that have patterns matching nothing.
    pub async fn drift_queue(&self, ctx: &Ctx, project: &str) -> Result<DriftQueue> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let snapshot = tx.source_snapshot(p.id).await?;
        let files = files_of(&snapshot);
        let mut entries = Vec::new();
        for d in tx.documents(p.id).await? {
            let doc = doc_entries(&mut tx, d.id, &d.path, d.title.as_deref(), &files).await?;
            entries.extend(doc.into_iter().filter(DriftEntry::needs_attention));
        }
        Ok(DriftQueue {
            revision: snapshot.as_ref().and_then(|s| s.revision.clone()),
            reported_at: snapshot.map(|s| s.reported_at),
            entries,
        })
    }

    /// Drift status of every bound section of one document.
    pub async fn doc_drift(&self, ctx: &Ctx, project: &str, path: &DocPath) -> Result<Vec<DriftEntry>> {
        let mut tx = self.tx(ctx).await?;
        let d = own_doc(&mut tx, ctx, project, path, Access::Read).await?;
        let files = files_of(&tx.source_snapshot(d.project_id).await?);
        doc_entries(&mut tx, d.id, &d.path, d.title.as_deref(), &files).await
    }

    /// Bound sections of `project`'s own documents whose patterns match any of
    /// `paths`. Works without a report.
    pub async fn affected_sections(&self, ctx: &Ctx, project: &str, paths: &[String]) -> Result<Vec<AffectedSection>> {
        for path in paths {
            validate_file(path, "-").map_err(invalid)?;
        }
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let mut out = Vec::new();
        for d in tx.documents(p.id).await? {
            let DocBindings { bindings, titles } = doc_bindings(&mut tx, d.id).await?;
            for b in bindings {
                let patterns = parse_patterns(&b.patterns);
                let hits: Vec<String> = paths
                    .iter()
                    .filter(|path| patterns.iter().any(|p| p.matches(path)))
                    .cloned()
                    .collect();
                if !hits.is_empty() {
                    out.push(AffectedSection {
                        path: d.path.clone(),
                        document_title: d.title.clone(),
                        section_title: titles.get(&b.anchor).cloned().unwrap_or_default(),
                        anchor: b.anchor,
                        paths: hits,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Bound sections of `project`'s own documents whose patterns match any of
    /// `paths`, with their content: the `prefer` variant, or the other variant when
    /// the section exists only there. Needs no source report.
    pub async fn section_context(
        &self,
        ctx: &Ctx,
        project: &str,
        paths: &[String],
        prefer: Variant,
    ) -> Result<Vec<SectionContext>> {
        for path in paths {
            validate_file(path, "-").map_err(invalid)?;
        }
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let mut out = Vec::new();
        for d in tx.documents(p.id).await? {
            let DocBindings { bindings, .. } = doc_bindings(&mut tx, d.id).await?;
            let bound: Vec<(SourceBinding, Vec<String>)> = bindings
                .into_iter()
                .filter_map(|b| {
                    let patterns = parse_patterns(&b.patterns);
                    let hits: Vec<String> = paths
                        .iter()
                        .filter(|path| patterns.iter().any(|p| p.matches(path)))
                        .cloned()
                        .collect();
                    (!hits.is_empty()).then_some((b, hits))
                })
                .collect();
            if bound.is_empty() {
                continue;
            }
            let base = DocPath::parse(&d.path).ok();
            let mut heads = Vec::new();
            for variant in [prefer, prefer.other()] {
                if let Some(h) = tx.head(d.id, variant).await? {
                    let a = analyze(&h.content, base.as_ref());
                    heads.push((h, a));
                }
            }
            for (b, hits) in bound {
                let Some((head, section)) = heads.iter().find_map(|(h, a)| a.section(&b.anchor).map(|s| (h, s))) else {
                    continue;
                };
                let links = analyze(&section.body, base.as_ref())
                    .links
                    .iter()
                    .filter(|t| t.path.is_some())
                    .map(|t| match &t.project {
                        Some(_) => t.to_string(),
                        None => format!("{}:{t}", p.slug),
                    })
                    .collect();
                out.push(SectionContext {
                    path: d.path.clone(),
                    document_title: d.title.clone(),
                    anchor: section.anchor.clone(),
                    section_title: section.title.clone(),
                    variant: head.variant,
                    content: section.body.clone(),
                    hash: section.hash,
                    content_hash: head.content_hash,
                    paths: hits,
                    links,
                });
            }
        }
        Ok(out)
    }

    /// Records that `anchors` still describe the files their patterns match in the
    /// current report. With `revision`, fails unless the current report has that
    /// revision. Returns the document's drift status.
    pub async fn verify_sources(
        &self,
        ctx: &Ctx,
        project: &str,
        path: &DocPath,
        anchors: &[String],
        revision: Option<&str>,
    ) -> Result<Vec<DriftEntry>> {
        if anchors.is_empty() {
            return Err(invalid("list the section anchors to verify"));
        }
        {
            let mut tx = self.tx(ctx).await?;
            let d = own_doc(&mut tx, ctx, project, path, Access::Write).await?;
            let snapshot = tx
                .source_snapshot(d.project_id)
                .await?
                .ok_or_else(|| invalid(format!("no source files have been reported for project {project}")))?;
            if let Some(rev) = revision
                && snapshot.revision.as_deref() != Some(rev)
            {
                return Err(invalid(format!(
                    "the current source report is at revision {}, not {rev}; re-check the drift queue",
                    snapshot.revision.as_deref().unwrap_or("(none)")
                )));
            }
            let bindings = doc_bindings(&mut tx, d.id).await?.bindings;
            let mut records = Vec::with_capacity(anchors.len());
            for anchor in anchors {
                let b = bindings
                    .iter()
                    .find(|b| &b.anchor == anchor)
                    .ok_or_else(|| invalid(format!("section #{anchor} has no source bindings")))?;
                records.push((anchor.clone(), matched(&parse_patterns(&b.patterns), &snapshot.files)));
            }
            tx.put_source_verifications(d.id, &records, snapshot.revision.as_deref(), ctx.author())
                .await?;
            let detail = json!({ "anchors": anchors, "revision": snapshot.revision });
            record(
                &mut tx,
                ctx,
                "sources.verify",
                Some(d.project_id),
                Some(&d.path),
                detail,
            )
            .await?;
            tx.commit().await?;
        }
        self.doc_drift(ctx, project, path).await
    }
}
