//! Translation proposals. The human and AI variants are two translations of one
//! document; when one side changes, an agent submits a replacement of the other side
//! as a proposal, and a reviewer accepts it (writing it and marking its `resolves`
//! sections in sync) or rejects it. The translation guide tells agents how the two
//! variants differ.

use std::collections::HashMap;
use std::collections::hash_map::Entry;

use serde_json::json;
use solidate_core::diff::unified;
use solidate_core::sync::{plan, section_hashes};
use solidate_core::{DocPath, Hash, Variant, analyze};
use solidate_db::{Expect, NewProposal, Proposal, ProposalId, TenantTx};

use crate::App;
use crate::audit::record;
use crate::ctx::{Access, Ctx};
use crate::docs::{PutDoc, PutResult, resolve};
use crate::error::{AppError, Result, invalid};
use crate::projects::project_by_slug;
use crate::sync::{Plan, doc_plan, own_doc};

/// Path of a project's translation guide. Resolved through inheritance, so a guide in
/// a parent project applies to its descendants.
pub const GUIDE_PATH: &str = "_meta/translation";

/// Guide used when no project in the inheritance chain defines one.
pub const DEFAULT_GUIDE: &str = "# Translation guide

Each document has a human variant and an AI variant. They describe the same reality \
for different readers and are translations of each other: every fact, decision and \
constraint in one must be in the other, and neither adds facts the other lacks.

## Human variant

Narrative prose for people. Explains context, reasoning and trade-offs, defines terms \
on first use, and uses complete sentences.

## AI variant

Dense, structured reference for agents. Bullet lists and `key: value` pairs; exact \
identifiers, paths, commands, limits and units; no narrative or repetition.

## Rules

- Keep the same sections in both variants so they pair up. Headings may be worded \
differently when both carry the same explicit anchor, e.g. `## Expiry {#expiry}`.
- Translate meaning, not wording: carry over every fact and change only the form.
- Do not resolve ambiguity by guessing. Keep it, and point it out in the proposal message.
- Keep links and `{{include}}` directives; they resolve per variant.
";

#[derive(Debug, Clone, serde::Serialize)]
pub struct TranslationGuide {
    /// `project:path` of the guide document; `None` for the built-in default.
    pub source: Option<String>,
    pub content: String,
}

pub struct Propose<'a> {
    pub project: &'a str,
    pub path: &'a DocPath,
    /// The variant the proposal replaces.
    pub variant: Variant,
    pub content: &'a str,
    /// Content hash of the variant being replaced; `None` when it does not exist yet.
    pub base: Option<Hash>,
    pub message: Option<&'a str>,
    /// Anchors the proposal translates. Empty: every section needing attention that
    /// the proposal leaves in the plan.
    pub resolves: &'a [String],
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProposalView {
    #[serde(flatten)]
    pub proposal: Proposal,
    /// A head moved since submission; the proposal can no longer be accepted.
    pub outdated: bool,
    /// Unified diff from the current variant to the proposed content.
    pub diff: String,
}

/// Proposal covering a section, for queue and status listings.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ProposalRef {
    pub id: ProposalId,
    pub variant: Variant,
    pub outdated: bool,
}

fn head_hash(plan: &Plan, v: Variant) -> Option<Hash> {
    match v {
        Variant::Human => plan.human.as_ref(),
        Variant::Ai => plan.ai.as_ref(),
    }
    .map(|h| h.content_hash)
}

fn outdated(p: &Proposal, plan: &Plan) -> bool {
    p.base_hash != head_hash(plan, p.variant) || p.source_hash != head_hash(plan, p.variant.other())
}

fn view(p: Proposal, plan: &Plan) -> ProposalView {
    let current = match p.variant {
        Variant::Human => plan.human.as_ref(),
        Variant::Ai => plan.ai.as_ref(),
    }
    .map_or("", |h| h.content.as_str());
    ProposalView {
        outdated: outdated(&p, plan),
        diff: unified(current, &p.content, "current", "proposed"),
        proposal: p,
    }
}

/// Proposals of a document keyed by the anchors they resolve. A current proposal
/// wins over an outdated one for the same anchor.
pub(crate) fn proposal_refs(proposals: &[Proposal], plan: &Plan) -> HashMap<String, ProposalRef> {
    let mut out: HashMap<String, ProposalRef> = HashMap::new();
    for p in proposals {
        let r = ProposalRef {
            id: p.id,
            variant: p.variant,
            outdated: outdated(p, plan),
        };
        for a in &p.resolves {
            if out.get(a).is_none_or(|x| x.outdated && !r.outdated) {
                out.insert(a.clone(), r);
            }
        }
    }
    out
}

async fn project_proposal(
    tx: &mut TenantTx,
    ctx: &Ctx,
    project: &str,
    id: ProposalId,
    access: Access,
) -> Result<(solidate_db::Project, Proposal)> {
    let p = project_by_slug(tx, project).await?;
    ctx.require(access, Some(&p))?;
    let proposal = tx.proposal(id).await?.ok_or(AppError::NotFound)?;
    let document = tx.document(proposal.document_id).await?.ok_or(AppError::NotFound)?;
    if document.project_id != p.id {
        return Err(AppError::NotFound);
    }
    Ok((p, proposal))
}

impl App {
    /// The translation guide in effect for `project`.
    pub async fn translation_guide(&self, ctx: &Ctx, project: &str) -> Result<TranslationGuide> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let chain = tx.project_chain(p.id).await?;
        let path = DocPath::parse(GUIDE_PATH).expect("valid guide path");
        if let Some((owner, doc)) = resolve(&mut tx, &chain, &path).await? {
            let head = match tx.head(doc.id, Variant::Ai).await? {
                Some(h) => Some(h),
                None => tx.head(doc.id, Variant::Human).await?,
            };
            if let Some(h) = head {
                return Ok(TranslationGuide {
                    source: Some(format!("{}:{GUIDE_PATH}", owner.slug)),
                    content: h.content,
                });
            }
        }
        Ok(TranslationGuide {
            source: None,
            content: DEFAULT_GUIDE.to_owned(),
        })
    }

    /// Submits a translation of one variant, replacing any open proposal for it.
    pub async fn propose(&self, ctx: &Ctx, req: Propose<'_>) -> Result<ProposalView> {
        if req.content.len() > self.config.max_doc_bytes {
            return Err(invalid(format!("document exceeds {} bytes", self.config.max_doc_bytes)));
        }
        let mut tx = self.tx(ctx).await?;
        let document = own_doc(&mut tx, ctx, req.project, req.path, Access::Write).await?;
        let current = doc_plan(&mut tx, document.id).await?;
        let (base, source) = (
            head_hash(&current, req.variant),
            head_hash(&current, req.variant.other()),
        );
        if source.is_none() {
            return Err(invalid(format!(
                "nothing to translate: the {} variant has not been written",
                req.variant.other()
            )));
        }
        if req.base != base {
            return Err(AppError::PreconditionFailed { current: base });
        }

        // Plan as it would be with the proposal applied, to check `resolves`.
        let analysis = analyze(req.content, Some(req.path));
        let proposed = section_hashes(&analysis.sections);
        let human: Vec<(&str, Hash)>;
        let ai: Vec<(&str, Hash)>;
        match req.variant {
            Variant::Human => {
                human = proposed;
                ai = current
                    .ai_sections
                    .iter()
                    .map(|s| (s.anchor.as_str(), s.hash))
                    .collect();
            }
            Variant::Ai => {
                human = current
                    .human_sections
                    .iter()
                    .map(|s| (s.anchor.as_str(), s.hash))
                    .collect();
                ai = proposed;
            }
        }
        let bases = tx.sync_bases(document.id).await?;
        let after = plan(&human, &ai, &bases);
        let resolves: Vec<String> = if req.resolves.is_empty() {
            current
                .plan
                .iter()
                .filter(|s| s.state.needs_attention() && after.iter().any(|x| x.anchor == s.anchor))
                .map(|s| s.anchor.clone())
                .collect()
        } else {
            if let Some(unknown) = req.resolves.iter().find(|a| !after.iter().any(|s| &s.anchor == *a)) {
                return Err(invalid(format!("unknown section anchor {unknown:?}")));
            }
            req.resolves.to_vec()
        };

        let id = tx
            .put_proposal(NewProposal {
                document: document.id,
                variant: req.variant,
                base_hash: base,
                source_hash: source,
                content: req.content,
                resolves: &resolves,
                message: req.message,
                author: ctx.author(),
            })
            .await?;
        let detail = json!({ "proposal": id, "variant": req.variant, "resolves": resolves });
        record(
            &mut tx,
            ctx,
            "proposal.submit",
            Some(document.project_id),
            Some(&document.path),
            detail,
        )
        .await?;
        let p = tx.proposal(id).await?.ok_or(AppError::NotFound)?;
        tx.commit().await?;
        Ok(view(p, &current))
    }

    /// Open proposals on one document of `project`.
    pub async fn doc_proposals(&self, ctx: &Ctx, project: &str, path: &DocPath) -> Result<Vec<ProposalView>> {
        let mut tx = self.tx(ctx).await?;
        let document = own_doc(&mut tx, ctx, project, path, Access::Read).await?;
        let plan = doc_plan(&mut tx, document.id).await?;
        let proposals = tx.document_proposals(document.id).await?;
        Ok(proposals.into_iter().map(|p| view(p, &plan)).collect())
    }

    /// Open proposals on `project`'s own documents.
    pub async fn project_proposals(&self, ctx: &Ctx, project: &str) -> Result<Vec<ProposalView>> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let mut out = Vec::new();
        let mut plans: HashMap<_, Plan> = HashMap::new();
        for proposal in tx.project_proposals(p.id).await? {
            let plan = match plans.entry(proposal.document_id) {
                Entry::Occupied(e) => e.into_mut(),
                Entry::Vacant(e) => e.insert(doc_plan(&mut tx, proposal.document_id).await?),
            };
            out.push(view(proposal, plan));
        }
        Ok(out)
    }

    /// Writes the proposed content as a new revision (authored by the caller) and
    /// marks its sections in sync. Fails with `PreconditionFailed` when outdated.
    pub async fn accept_proposal(&self, ctx: &Ctx, project: &str, id: ProposalId) -> Result<PutResult> {
        let mut tx = self.tx(ctx).await?;
        let (p, proposal) = project_proposal(&mut tx, ctx, project, id, Access::Write).await?;
        let plan = doc_plan(&mut tx, proposal.document_id).await?;
        if outdated(&proposal, &plan) {
            return Err(AppError::PreconditionFailed {
                current: head_hash(&plan, proposal.variant),
            });
        }
        let path = DocPath::parse(&proposal.path).map_err(|e| AppError::Internal(e.to_string()))?;
        let message = proposal.message.as_deref().unwrap_or("Accept translation proposal");
        let r = self
            .put_doc_tx(
                &mut tx,
                ctx,
                PutDoc {
                    project,
                    path: &path,
                    variant: proposal.variant,
                    content: &proposal.content,
                    expect: proposal.base_hash.map_or(Expect::Absent, Expect::Head),
                    message: Some(message),
                    resolves: &proposal.resolves,
                },
            )
            .await?;
        tx.delete_proposal(id).await?;
        let proposer = match (proposal.author_user_id, proposal.author_token_id) {
            (Some(u), _) => json!({ "user": u }),
            (_, Some(t)) => json!({ "token": t }),
            _ => json!("system"),
        };
        let detail = json!({
            "proposal": id,
            "variant": proposal.variant,
            "revision": r.revision.id,
            "proposer": proposer,
        });
        record(
            &mut tx,
            ctx,
            "proposal.accept",
            Some(p.id),
            Some(&proposal.path),
            detail,
        )
        .await?;
        tx.commit().await?;
        Ok(r)
    }

    /// Deletes a proposal. Returns the path of its document.
    pub async fn reject_proposal(&self, ctx: &Ctx, project: &str, id: ProposalId) -> Result<String> {
        let mut tx = self.tx(ctx).await?;
        let (p, proposal) = project_proposal(&mut tx, ctx, project, id, Access::Write).await?;
        tx.delete_proposal(id).await?;
        let detail = json!({ "proposal": id, "variant": proposal.variant });
        record(
            &mut tx,
            ctx,
            "proposal.reject",
            Some(p.id),
            Some(&proposal.path),
            detail,
        )
        .await?;
        tx.commit().await?;
        Ok(proposal.path)
    }
}
