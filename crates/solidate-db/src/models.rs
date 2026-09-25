//! Row types.

use serde::Serialize;
use solidate_core::{Hash, Role, Scope, Variant};
use time::OffsetDateTime;

use crate::ids::*;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Tenant {
    pub id: TenantId,
    pub slug: String,
    pub name: String,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct TenantMembership {
    #[sqlx(flatten)]
    pub tenant: Tenant,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct User {
    pub id: UserId,
    pub email: String,
    pub name: String,
    pub password_hash: String,
    pub created_at: OffsetDateTime,
    pub disabled_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Member {
    pub user_id: UserId,
    pub email: String,
    pub name: String,
    pub role: Role,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Project {
    pub id: ProjectId,
    pub slug: String,
    pub name: String,
    pub parent_id: Option<ProjectId>,
    pub settings: serde_json::Value,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Document {
    pub id: DocumentId,
    pub project_id: ProjectId,
    pub path: String,
    pub title: Option<String>,
    pub sync_enabled: bool,
    pub created_at: OffsetDateTime,
    pub updated_at: OffsetDateTime,
}

/// A document with the content hash of each variant's head.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct DocumentSummary {
    pub id: DocumentId,
    pub path: String,
    pub title: Option<String>,
    pub sync_enabled: bool,
    pub human_hash: Option<Hash>,
    pub ai_hash: Option<Hash>,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Head {
    pub document_id: DocumentId,
    pub variant: Variant,
    pub revision_id: RevisionId,
    pub content_hash: Hash,
    pub title: Option<String>,
    pub content: String,
    pub updated_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Revision {
    pub id: RevisionId,
    pub document_id: DocumentId,
    pub variant: Variant,
    pub content_hash: Hash,
    pub semantic_hash: Hash,
    pub parent_id: Option<RevisionId>,
    pub author_user_id: Option<UserId>,
    pub author_token_id: Option<TokenId>,
    pub message: Option<String>,
    pub created_at: OffsetDateTime,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct SectionRow {
    pub ordinal: i32,
    pub anchor: String,
    pub title: String,
    pub level: i16,
    pub parent_anchor: Option<String>,
    pub hash: Hash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct Backlink {
    pub document_id: DocumentId,
    pub project_slug: String,
    pub path: String,
    pub variant: Variant,
    pub target_anchor: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, sqlx::FromRow)]
pub struct SearchHit {
    pub document_id: DocumentId,
    pub project_slug: String,
    pub path: String,
    pub title: Option<String>,
    pub variant: Variant,
    pub rank: f32,
    /// `ts_headline` output, unescaped. Matches are delimited by
    /// [`SNIPPET_START`] and [`SNIPPET_END`]; use [`SearchHit::snippet_html`] for HTML.
    pub snippet: String,
}

/// Match delimiters in [`SearchHit::snippet`] (Unicode private-use characters).
pub const SNIPPET_START: char = '\u{E000}';
pub const SNIPPET_END: char = '\u{E001}';

impl SearchHit {
    /// The snippet HTML-escaped, with matches wrapped in `<mark>`.
    pub fn snippet_html(&self) -> String {
        let mut out = String::with_capacity(self.snippet.len() + 16);
        for c in self.snippet.chars() {
            match c {
                SNIPPET_START => out.push_str("<mark>"),
                SNIPPET_END => out.push_str("</mark>"),
                '&' => out.push_str("&amp;"),
                '<' => out.push_str("&lt;"),
                '>' => out.push_str("&gt;"),
                '"' => out.push_str("&quot;"),
                '\'' => out.push_str("&#39;"),
                c => out.push(c),
            }
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, sqlx::FromRow)]
pub struct ApiToken {
    pub id: TokenId,
    pub name: String,
    pub prefix: String,
    pub scopes: Vec<Scope>,
    pub project_id: Option<ProjectId>,
    pub created_by: Option<UserId>,
    pub created_at: OffsetDateTime,
    pub expires_at: Option<OffsetDateTime>,
    pub last_used_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

/// Token row as returned by the pre-tenant lookup.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct TokenCredentials {
    pub id: TokenId,
    pub tenant_id: TenantId,
    pub secret_hash: Vec<u8>,
    pub scopes: Vec<Scope>,
    pub project_id: Option<ProjectId>,
    pub expires_at: Option<OffsetDateTime>,
    pub revoked_at: Option<OffsetDateTime>,
}

#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SessionRecord {
    #[sqlx(flatten)]
    pub user: User,
    pub expires_at: OffsetDateTime,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snippet_html_escapes_text_and_marks_matches() {
        let hit = SearchHit {
            document_id: DocumentId::new(),
            project_slug: "p".into(),
            path: "x".into(),
            title: None,
            variant: Variant::Human,
            rank: 1.0,
            snippet: format!("<script>{SNIPPET_START}rotated{SNIPPET_END} & \"x\""),
        };
        assert_eq!(
            hit.snippet_html(),
            "&lt;script&gt;<mark>rotated</mark> &amp; &quot;x&quot;"
        );
    }
}
