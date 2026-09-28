//! Reported source files and section verifications (see migration `0006_sources.sql`).

use std::collections::HashMap;

use solidate_core::sources::Files;
use sqlx::types::Json;
use time::OffsetDateTime;

use crate::documents::Author;
use crate::ids::*;
use crate::models::*;
use crate::{Result, TenantTx};

impl TenantTx {
    /// The project's last report with all of its files, or `None` if nothing was reported.
    pub async fn source_snapshot(&mut self, project: ProjectId) -> Result<Option<SourceSnapshot>> {
        let Some((revision, reported_at)): Option<(Option<String>, OffsetDateTime)> =
            sqlx::query_as("SELECT revision, reported_at FROM source_snapshots WHERE project_id = $1")
                .bind(project)
                .fetch_optional(self.conn())
                .await?
        else {
            return Ok(None);
        };
        let rows: Vec<(String, String)> = sqlx::query_as("SELECT path, hash FROM source_files WHERE project_id = $1")
            .bind(project)
            .fetch_all(self.conn())
            .await?;
        Ok(Some(SourceSnapshot {
            revision,
            reported_at,
            files: rows.into_iter().collect(),
        }))
    }

    /// Records a report. With `replace`, files not in `files` are removed first;
    /// otherwise `files` are upserted and `removed` deleted. Returns the number of
    /// files the project has afterwards.
    pub async fn put_source_files(
        &mut self,
        project: ProjectId,
        revision: Option<&str>,
        files: &Files,
        removed: &[String],
        replace: bool,
    ) -> Result<i64> {
        sqlx::query(
            "INSERT INTO source_snapshots (project_id, revision) VALUES ($1, $2)
             ON CONFLICT (tenant_id, project_id) DO UPDATE SET revision = EXCLUDED.revision, reported_at = now()",
        )
        .bind(project)
        .bind(revision)
        .execute(self.conn())
        .await?;
        if replace {
            sqlx::query("DELETE FROM source_files WHERE project_id = $1")
                .bind(project)
                .execute(self.conn())
                .await?;
        } else {
            sqlx::query("DELETE FROM source_files WHERE project_id = $1 AND path = ANY($2)")
                .bind(project)
                .bind(removed)
                .execute(self.conn())
                .await?;
        }
        sqlx::query(
            "INSERT INTO source_files (project_id, path, hash)
             SELECT $1, * FROM UNNEST($2::text[], $3::text[])
             ON CONFLICT (tenant_id, project_id, path) DO UPDATE SET hash = EXCLUDED.hash",
        )
        .bind(project)
        .bind(files.keys().cloned().collect::<Vec<_>>())
        .bind(files.values().cloned().collect::<Vec<_>>())
        .execute(self.conn())
        .await?;
        Ok(
            sqlx::query_scalar("SELECT count(*) FROM source_files WHERE project_id = $1")
                .bind(project)
                .fetch_one(self.conn())
                .await?,
        )
    }

    /// Verification records of a document, by anchor.
    pub async fn source_verifications(&mut self, document: DocumentId) -> Result<HashMap<String, SourceVerification>> {
        let rows: Vec<SourceVerification> = sqlx::query_as(
            "SELECT anchor, files, revision, author_user_id, author_token_id, verified_at
             FROM source_verifications WHERE document_id = $1",
        )
        .bind(document)
        .fetch_all(self.conn())
        .await?;
        Ok(rows.into_iter().map(|v| (v.anchor.clone(), v)).collect())
    }

    /// Upserts one verification record per `(anchor, files)`.
    pub async fn put_source_verifications(
        &mut self,
        document: DocumentId,
        records: &[(String, Files)],
        revision: Option<&str>,
        author: Author,
    ) -> Result<()> {
        let (user, token) = match author {
            Author::User(u) => (Some(u), None),
            Author::Token(t) => (None, Some(t)),
            Author::System => (None, None),
        };
        sqlx::query(
            "INSERT INTO source_verifications (document_id, anchor, files, revision, author_user_id, author_token_id)
             SELECT $1, a, f, $4, $5, $6 FROM UNNEST($2::text[], $3::jsonb[]) AS u (a, f)
             ON CONFLICT (tenant_id, document_id, anchor) DO UPDATE SET
                 files = EXCLUDED.files, revision = EXCLUDED.revision, author_user_id = EXCLUDED.author_user_id,
                 author_token_id = EXCLUDED.author_token_id, verified_at = now()",
        )
        .bind(document)
        .bind(records.iter().map(|(a, _)| a.clone()).collect::<Vec<_>>())
        .bind(records.iter().map(|(_, f)| Json(f)).collect::<Vec<_>>())
        .bind(revision)
        .bind(user)
        .bind(token)
        .execute(self.conn())
        .await?;
        Ok(())
    }

    /// Removes verification records whose anchors are not in `keep`.
    pub async fn prune_source_verifications(&mut self, document: DocumentId, keep: &[String]) -> Result<u64> {
        Ok(
            sqlx::query("DELETE FROM source_verifications WHERE document_id = $1 AND NOT (anchor = ANY($2))")
                .bind(document)
                .bind(keep)
                .execute(self.conn())
                .await?
                .rows_affected(),
        )
    }
}
