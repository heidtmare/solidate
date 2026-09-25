//! Documents, revisions, heads, sections and links.

use solidate_core::{Analysis, DocPath, Hash, Variant};

use crate::ids::*;
use crate::models::*;
use crate::{DbError, Result, TenantTx};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Author {
    User(UserId),
    Token(TokenId),
    System,
}

/// Precondition on the current head, for optimistic concurrency (`If-Match`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Any,
    /// The variant must not have a head yet.
    Absent,
    /// The head's content hash must equal this.
    Head(Hash),
}

pub struct NewRevision<'a> {
    pub document: DocumentId,
    pub variant: Variant,
    pub content: &'a str,
    /// Result of [`solidate_core::analyze`] on `content`.
    pub analysis: &'a Analysis,
    pub author: Author,
    pub message: Option<&'a str>,
    pub expect: Expect,
}

macro_rules! head_select {
    ($where:literal) => {
        concat!(
            "SELECT h.document_id, h.variant, h.revision_id, h.content_hash, h.title, b.content, h.updated_at
             FROM heads h JOIN blobs b ON b.tenant_id = h.tenant_id AND b.hash = h.content_hash ",
            $where
        )
    };
}

impl TenantTx {
    pub async fn create_document(&mut self, project: ProjectId, path: &DocPath) -> Result<Document> {
        Ok(
            sqlx::query_as("INSERT INTO documents (id, project_id, path) VALUES ($1, $2, $3) RETURNING *")
                .bind(DocumentId::new())
                .bind(project)
                .bind(path.as_str())
                .fetch_one(self.conn())
                .await?,
        )
    }

    pub async fn document(&mut self, id: DocumentId) -> Result<Option<Document>> {
        Ok(
            sqlx::query_as("SELECT * FROM documents WHERE id = $1 AND deleted_at IS NULL")
                .bind(id)
                .fetch_optional(self.conn())
                .await?,
        )
    }

    pub async fn document_by_path(&mut self, project: ProjectId, path: &DocPath) -> Result<Option<Document>> {
        Ok(
            sqlx::query_as("SELECT * FROM documents WHERE project_id = $1 AND path = $2 AND deleted_at IS NULL")
                .bind(project)
                .bind(path.as_str())
                .fetch_optional(self.conn())
                .await?,
        )
    }

    /// Live documents of `project`, ordered by path.
    pub async fn documents(&mut self, project: ProjectId) -> Result<Vec<DocumentSummary>> {
        Ok(sqlx::query_as(
            "SELECT d.id, d.path, d.title, d.sync_enabled, h.content_hash AS human_hash,
                    a.content_hash AS ai_hash, d.updated_at
             FROM documents d
             LEFT JOIN heads h ON h.tenant_id = d.tenant_id AND h.document_id = d.id AND h.variant = 'human'
             LEFT JOIN heads a ON a.tenant_id = d.tenant_id AND a.document_id = d.id AND a.variant = 'ai'
             WHERE d.project_id = $1 AND d.deleted_at IS NULL
             ORDER BY d.path",
        )
        .bind(project)
        .fetch_all(self.conn())
        .await?)
    }

    pub async fn delete_document(&mut self, id: DocumentId) -> Result<()> {
        let n = sqlx::query("UPDATE documents SET deleted_at = now() WHERE id = $1 AND deleted_at IS NULL")
            .bind(id)
            .execute(self.conn())
            .await?
            .rows_affected();
        if n == 0 { Err(DbError::NotFound) } else { Ok(()) }
    }

    pub async fn set_sync_enabled(&mut self, id: DocumentId, enabled: bool) -> Result<()> {
        sqlx::query("UPDATE documents SET sync_enabled = $2 WHERE id = $1")
            .bind(id)
            .bind(enabled)
            .execute(self.conn())
            .await?;
        Ok(())
    }

    pub async fn head(&mut self, document: DocumentId, variant: Variant) -> Result<Option<Head>> {
        Ok(
            sqlx::query_as(head_select!("WHERE h.document_id = $1 AND h.variant = $2"))
                .bind(document)
                .bind(variant)
                .fetch_optional(self.conn())
                .await?,
        )
    }

    /// Writes a new revision and moves the variant's head to it.
    ///
    /// Returns the head revision and whether a new one was created. Writing content
    /// identical to the current head creates nothing.
    pub async fn write_revision(&mut self, rev: NewRevision<'_>) -> Result<(Revision, bool)> {
        let a = rev.analysis;
        let current: Option<(RevisionId, Hash)> = sqlx::query_as(
            "SELECT revision_id, content_hash FROM heads WHERE document_id = $1 AND variant = $2 FOR UPDATE",
        )
        .bind(rev.document)
        .bind(rev.variant)
        .fetch_optional(self.conn())
        .await?;

        let current_hash = current.map(|(_, h)| h);
        let ok = match rev.expect {
            Expect::Any => true,
            Expect::Absent => current.is_none(),
            Expect::Head(h) => current_hash == Some(h),
        };
        if !ok {
            return Err(DbError::HeadMismatch { current: current_hash });
        }
        if let Some((rev_id, hash)) = current
            && hash == a.content_hash
        {
            return Ok((self.revision(rev_id).await?.ok_or(DbError::NotFound)?, false));
        }

        sqlx::query("INSERT INTO blobs (hash, content, byte_len) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING")
            .bind(a.content_hash)
            .bind(rev.content)
            .bind(rev.content.len() as i32)
            .execute(self.conn())
            .await?;

        let (user, token) = match rev.author {
            Author::User(u) => (Some(u), None),
            Author::Token(t) => (None, Some(t)),
            Author::System => (None, None),
        };
        let revision: Revision = sqlx::query_as(
            "INSERT INTO revisions (id, document_id, variant, content_hash, semantic_hash, parent_id,
                                    author_user_id, author_token_id, message)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
             RETURNING id, document_id, variant, content_hash, semantic_hash, parent_id,
                       author_user_id, author_token_id, message, created_at",
        )
        .bind(RevisionId::new())
        .bind(rev.document)
        .bind(rev.variant)
        .bind(a.content_hash)
        .bind(a.semantic_hash)
        .bind(current.map(|(id, _)| id))
        .bind(user)
        .bind(token)
        .bind(rev.message)
        .fetch_one(self.conn())
        .await?;

        let path = self.document_path(rev.document).await?;
        let search_title = format!(
            "{} {}",
            a.title.as_deref().unwrap_or(""),
            path.replace(['/', '-', '_'], " ")
        );
        let head_sql = if current.is_some() {
            "UPDATE heads SET revision_id = $3, content_hash = $4, title = $5,
                 search = setweight(to_tsvector('english', $6), 'A') || setweight(to_tsvector('english', $7), 'B'),
                 updated_at = now()
             WHERE document_id = $1 AND variant = $2"
        } else {
            "INSERT INTO heads (document_id, variant, revision_id, content_hash, title, search)
             VALUES ($1, $2, $3, $4, $5,
                     setweight(to_tsvector('english', $6), 'A') || setweight(to_tsvector('english', $7), 'B'))"
        };
        let head = sqlx::query(head_sql)
            .bind(rev.document)
            .bind(rev.variant)
            .bind(revision.id)
            .bind(a.content_hash)
            .bind(a.title.as_deref())
            .bind(&search_title)
            .bind(rev.content)
            .execute(self.conn())
            .await;
        match head {
            Err(sqlx::Error::Database(d)) if d.is_unique_violation() => {
                return Err(DbError::HeadMismatch { current: None });
            }
            r => {
                r?;
            }
        }

        self.insert_sections(revision.id, a).await?;
        self.replace_links(rev.document, rev.variant, a).await?;

        sqlx::query(
            "UPDATE documents SET updated_at = now(), title = coalesce(
                 (SELECT title FROM heads WHERE document_id = $1 AND variant = 'human'),
                 (SELECT title FROM heads WHERE document_id = $1 AND variant = 'ai'))
             WHERE id = $1",
        )
        .bind(rev.document)
        .execute(self.conn())
        .await?;

        Ok((revision, true))
    }

    async fn document_path(&mut self, id: DocumentId) -> Result<String> {
        Ok(sqlx::query_scalar("SELECT path FROM documents WHERE id = $1")
            .bind(id)
            .fetch_one(self.conn())
            .await?)
    }

    async fn insert_sections(&mut self, revision: RevisionId, a: &Analysis) -> Result<()> {
        let s = &a.sections;
        sqlx::query(
            "INSERT INTO sections (revision_id, ordinal, anchor, title, level, parent_anchor, hash)
             SELECT $1, * FROM UNNEST($2::int4[], $3::text[], $4::text[], $5::int2[], $6::text[], $7::bytea[])",
        )
        .bind(revision)
        .bind(s.iter().map(|x| x.ordinal as i32).collect::<Vec<_>>())
        .bind(s.iter().map(|x| x.anchor.clone()).collect::<Vec<_>>())
        .bind(s.iter().map(|x| x.title.clone()).collect::<Vec<_>>())
        .bind(s.iter().map(|x| x.level as i16).collect::<Vec<_>>())
        .bind(s.iter().map(|x| x.parent.clone()).collect::<Vec<_>>())
        .bind(s.iter().map(|x| x.hash).collect::<Vec<_>>())
        .execute(self.conn())
        .await?;
        Ok(())
    }

    async fn replace_links(&mut self, document: DocumentId, variant: Variant, a: &Analysis) -> Result<()> {
        sqlx::query("DELETE FROM links WHERE document_id = $1 AND variant = $2")
            .bind(document)
            .bind(variant)
            .execute(self.conn())
            .await?;
        let l = &a.links;
        sqlx::query(
            "INSERT INTO links (document_id, variant, ordinal, target_project, target_path, target_anchor)
             SELECT $1, $2, * FROM UNNEST($3::int4[], $4::text[], $5::text[], $6::text[])",
        )
        .bind(document)
        .bind(variant)
        .bind((0..l.len() as i32).collect::<Vec<_>>())
        .bind(
            l.iter()
                .map(|t| t.project.as_ref().map(ToString::to_string))
                .collect::<Vec<_>>(),
        )
        .bind(
            l.iter()
                .map(|t| t.path.as_ref().map(ToString::to_string))
                .collect::<Vec<_>>(),
        )
        .bind(l.iter().map(|t| t.anchor.clone()).collect::<Vec<_>>())
        .execute(self.conn())
        .await?;
        Ok(())
    }

    pub async fn revision(&mut self, id: RevisionId) -> Result<Option<Revision>> {
        Ok(sqlx::query_as(
            "SELECT id, document_id, variant, content_hash, semantic_hash, parent_id,
                    author_user_id, author_token_id, message, created_at
             FROM revisions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(self.conn())
        .await?)
    }

    /// Newest first.
    pub async fn revisions(&mut self, document: DocumentId, variant: Variant, limit: i64) -> Result<Vec<Revision>> {
        Ok(sqlx::query_as(
            "SELECT id, document_id, variant, content_hash, semantic_hash, parent_id,
                    author_user_id, author_token_id, message, created_at
             FROM revisions WHERE document_id = $1 AND variant = $2
             ORDER BY created_at DESC, id DESC LIMIT $3",
        )
        .bind(document)
        .bind(variant)
        .bind(limit)
        .fetch_all(self.conn())
        .await?)
    }

    pub async fn blob(&mut self, hash: Hash) -> Result<Option<String>> {
        Ok(sqlx::query_scalar("SELECT content FROM blobs WHERE hash = $1")
            .bind(hash)
            .fetch_optional(self.conn())
            .await?)
    }

    /// Content hash of the newest revision of `variant` that contains section
    /// `anchor` with semantic hash `hash`.
    pub async fn content_with_section(
        &mut self,
        document: DocumentId,
        variant: Variant,
        anchor: &str,
        hash: Hash,
    ) -> Result<Option<Hash>> {
        Ok(sqlx::query_scalar(
            "SELECT r.content_hash FROM sections s
             JOIN revisions r ON r.tenant_id = s.tenant_id AND r.id = s.revision_id
             WHERE r.document_id = $1 AND r.variant = $2 AND s.anchor = $3 AND s.hash = $4
             ORDER BY r.created_at DESC, r.id DESC LIMIT 1",
        )
        .bind(document)
        .bind(variant)
        .bind(anchor)
        .bind(hash)
        .fetch_optional(self.conn())
        .await?)
    }

    pub async fn sections(&mut self, revision: RevisionId) -> Result<Vec<SectionRow>> {
        Ok(sqlx::query_as(
            "SELECT ordinal, anchor, title, level, parent_anchor, hash
             FROM sections WHERE revision_id = $1 ORDER BY ordinal",
        )
        .bind(revision)
        .fetch_all(self.conn())
        .await?)
    }

    /// Live documents linking to `path` in `project`, from either variant.
    pub async fn backlinks(&mut self, project: &Project, path: &DocPath) -> Result<Vec<Backlink>> {
        Ok(sqlx::query_as(
            "SELECT DISTINCT d.id AS document_id, p.slug AS project_slug, d.path, l.variant, l.target_anchor
             FROM links l
             JOIN documents d ON d.tenant_id = l.tenant_id AND d.id = l.document_id
             JOIN projects p ON p.tenant_id = d.tenant_id AND p.id = d.project_id
             WHERE l.target_path = $1
               AND ((l.target_project IS NULL AND d.project_id = $2) OR l.target_project = $3)
               AND d.deleted_at IS NULL
             ORDER BY p.slug, d.path",
        )
        .bind(path.as_str())
        .bind(project.id)
        .bind(&project.slug)
        .fetch_all(self.conn())
        .await?)
    }
}
