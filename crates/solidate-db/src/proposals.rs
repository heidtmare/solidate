//! Translation proposals (see migration `0003_proposals.sql`).

use solidate_core::{Hash, Variant};

use crate::documents::Author;
use crate::ids::*;
use crate::models::*;
use crate::{Result, TenantTx};

pub struct NewProposal<'a> {
    pub document: DocumentId,
    pub variant: Variant,
    pub base_hash: Option<Hash>,
    pub source_hash: Option<Hash>,
    pub content: &'a str,
    pub resolves: &'a [String],
    pub message: Option<&'a str>,
    pub author: Author,
}

macro_rules! proposal_select {
    ($where:literal) => {
        concat!(
            "SELECT p.id, p.document_id, d.path, p.variant, p.base_hash, p.source_hash, p.content, p.resolves,
                    p.message, p.author_user_id, p.author_token_id, p.created_at
             FROM proposals p JOIN documents d ON d.tenant_id = p.tenant_id AND d.id = p.document_id
             WHERE d.deleted_at IS NULL AND ",
            $where
        )
    };
}

impl TenantTx {
    /// Inserts a proposal, replacing any existing one for the same document variant.
    pub async fn put_proposal(&mut self, p: NewProposal<'_>) -> Result<ProposalId> {
        let (user, token) = match p.author {
            Author::User(u) => (Some(u), None),
            Author::Token(t) => (None, Some(t)),
            Author::System => (None, None),
        };
        let (id,): (ProposalId,) = sqlx::query_as(
            "INSERT INTO proposals (id, document_id, variant, base_hash, source_hash, content, resolves, message,
                                    author_user_id, author_token_id)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)
             ON CONFLICT (tenant_id, document_id, variant) DO UPDATE SET
                 id = EXCLUDED.id, base_hash = EXCLUDED.base_hash, source_hash = EXCLUDED.source_hash,
                 content = EXCLUDED.content, resolves = EXCLUDED.resolves, message = EXCLUDED.message,
                 author_user_id = EXCLUDED.author_user_id, author_token_id = EXCLUDED.author_token_id,
                 created_at = now()
             RETURNING id",
        )
        .bind(ProposalId::new())
        .bind(p.document)
        .bind(p.variant)
        .bind(p.base_hash)
        .bind(p.source_hash)
        .bind(p.content)
        .bind(p.resolves)
        .bind(p.message)
        .bind(user)
        .bind(token)
        .fetch_one(self.conn())
        .await?;
        Ok(id)
    }

    pub async fn proposal(&mut self, id: ProposalId) -> Result<Option<Proposal>> {
        Ok(sqlx::query_as(proposal_select!("p.id = $1"))
            .bind(id)
            .fetch_optional(self.conn())
            .await?)
    }

    pub async fn document_proposals(&mut self, document: DocumentId) -> Result<Vec<Proposal>> {
        Ok(
            sqlx::query_as(proposal_select!("p.document_id = $1 ORDER BY p.variant"))
                .bind(document)
                .fetch_all(self.conn())
                .await?,
        )
    }

    /// Proposals on live documents owned by `project`, ordered by path and variant.
    pub async fn project_proposals(&mut self, project: ProjectId) -> Result<Vec<Proposal>> {
        Ok(
            sqlx::query_as(proposal_select!("d.project_id = $1 ORDER BY d.path, p.variant"))
                .bind(project)
                .fetch_all(self.conn())
                .await?,
        )
    }

    pub async fn delete_proposal(&mut self, id: ProposalId) -> Result<bool> {
        Ok(sqlx::query("DELETE FROM proposals WHERE id = $1")
            .bind(id)
            .execute(self.conn())
            .await?
            .rows_affected()
            > 0)
    }

    /// Deletes the proposal for one document variant, if any.
    pub async fn delete_variant_proposal(&mut self, document: DocumentId, variant: Variant) -> Result<bool> {
        Ok(
            sqlx::query("DELETE FROM proposals WHERE document_id = $1 AND variant = $2")
                .bind(document)
                .bind(variant)
                .execute(self.conn())
                .await?
                .rows_affected()
                > 0,
        )
    }
}
