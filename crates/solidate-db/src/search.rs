//! Full-text search over document heads, resolved to sections.

use solidate_core::Hash;
use solidate_core::markdown::analyze;
use solidate_core::sync::Variant;

use crate::ids::*;
use crate::models::*;
use crate::{Result, TenantTx};

/// `ts_headline` options shared by document and section snippets (SQL expression).
macro_rules! headline_opts {
    () => {
        "'StartSel=' || chr(57344) || ', StopSel=' || chr(57345) || ', MaxFragments=2, MaxWords=20, MinWords=5'"
    };
}

#[derive(sqlx::FromRow)]
struct DocHit {
    document_id: DocumentId,
    project_slug: String,
    path: String,
    title: Option<String>,
    variant: Variant,
    rank: f32,
    snippet: String,
    content: String,
}

#[derive(sqlx::FromRow)]
struct SectionRank {
    /// 0-based index into the submitted section bodies.
    idx: i64,
    rank: f32,
    snippet: String,
}

impl TenantTx {
    /// Searches live documents using `websearch_to_tsquery` syntax, restricted to
    /// `project` and `variant` when given. Matching documents (at most `limit`, by
    /// document rank) are split into sections; each matching section is one hit,
    /// ordered by document rank, then section rank. A document that matches only
    /// through its title or path yields one hit without an anchor. At most `limit`
    /// hits are returned.
    pub async fn search(
        &mut self,
        query: &str,
        project: Option<ProjectId>,
        variant: Option<Variant>,
        limit: i64,
    ) -> Result<Vec<SearchHit>> {
        let docs: Vec<DocHit> = sqlx::query_as(concat!(
            "WITH q AS (SELECT websearch_to_tsquery('english', $1) AS q)
             SELECT d.id AS document_id, p.slug AS project_slug, d.path, h.title, h.variant,
                    ts_rank(h.search, q.q) AS rank,
                    ts_headline('english', b.content, q.q, ",
            headline_opts!(),
            ") AS snippet,
                    b.content
             FROM heads h
             CROSS JOIN q
             JOIN documents d ON d.tenant_id = h.tenant_id AND d.id = h.document_id
             JOIN projects p ON p.tenant_id = d.tenant_id AND p.id = d.project_id
             JOIN blobs b ON b.tenant_id = h.tenant_id AND b.hash = h.content_hash
             WHERE h.search @@ q.q AND d.deleted_at IS NULL
               AND ($2::uuid IS NULL OR d.project_id = $2)
               AND ($3::text IS NULL OR h.variant = $3)
             ORDER BY rank DESC, d.path
             LIMIT $4"
        ))
        .bind(query)
        .bind(project)
        .bind(variant)
        .bind(limit)
        .fetch_all(self.conn())
        .await?;

        let analyses: Vec<_> = docs.iter().map(|d| analyze(&d.content, None)).collect();
        // Flattened (document index, section index) for every section of every hit.
        let refs: Vec<(usize, usize)> = analyses
            .iter()
            .enumerate()
            .flat_map(|(di, a)| (0..a.sections.len()).map(move |si| (di, si)))
            .collect();
        let bodies: Vec<&str> = refs
            .iter()
            .map(|&(di, si)| analyses[di].sections[si].body.as_str())
            .collect();
        let ranks = self.rank_sections(query, &bodies).await?;

        let mut per_doc: Vec<Vec<(f32, usize, String)>> = vec![Vec::new(); docs.len()];
        for r in ranks {
            let (di, si) = refs[r.idx as usize];
            per_doc[di].push((r.rank, si, r.snippet));
        }

        let mut hits = Vec::new();
        for ((doc, a), mut sections) in docs.into_iter().zip(&analyses).zip(per_doc) {
            let hit = |anchor: Option<String>, section_title: Option<String>, section_hash: Option<Hash>, snippet| {
                SearchHit {
                    document_id: doc.document_id,
                    project_slug: doc.project_slug.clone(),
                    path: doc.path.clone(),
                    title: doc.title.clone(),
                    variant: doc.variant,
                    rank: doc.rank,
                    anchor,
                    section_title,
                    section_hash,
                    snippet,
                }
            };
            if sections.is_empty() {
                hits.push(hit(None, None, None, doc.snippet.clone()));
                continue;
            }
            sections.sort_by(|x, y| y.0.total_cmp(&x.0).then(x.1.cmp(&y.1)));
            for (_, si, snippet) in sections {
                let s = &a.sections[si];
                hits.push(hit(
                    Some(s.anchor.clone()),
                    Some(s.title.clone()),
                    Some(s.hash),
                    snippet,
                ));
            }
        }
        hits.truncate(limit.max(0) as usize);
        Ok(hits)
    }

    /// Ranks `bodies` against `query`; only matching bodies are returned.
    async fn rank_sections(&mut self, query: &str, bodies: &[&str]) -> Result<Vec<SectionRank>> {
        if bodies.is_empty() {
            return Ok(Vec::new());
        }
        Ok(sqlx::query_as(concat!(
            "WITH q AS (SELECT websearch_to_tsquery('english', $1) AS q)
             SELECT s.i - 1 AS idx, ts_rank(to_tsvector('english', s.body), q.q) AS rank,
                    ts_headline('english', s.body, q.q, ",
            headline_opts!(),
            ") AS snippet
             FROM unnest($2::text[]) WITH ORDINALITY AS s(body, i)
             CROSS JOIN q
             WHERE to_tsvector('english', s.body) @@ q.q"
        ))
        .bind(query)
        .bind(bodies)
        .fetch_all(self.conn())
        .await?)
    }
}
