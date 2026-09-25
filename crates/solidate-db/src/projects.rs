//! Projects, the inheritance tree, and memberships.

use solidate_core::{Role, Slug};

use crate::ids::*;
use crate::models::*;
use crate::{DbError, Result, TenantTx};

impl TenantTx {
    /// The tenant this transaction is scoped to.
    pub async fn tenant_row(&mut self) -> Result<Tenant> {
        Ok(sqlx::query_as("SELECT * FROM tenants WHERE id = current_tenant()")
            .fetch_one(self.conn())
            .await?)
    }

    pub async fn create_project(&mut self, slug: &Slug, name: &str, parent: Option<ProjectId>) -> Result<Project> {
        Ok(
            sqlx::query_as("INSERT INTO projects (id, slug, name, parent_id) VALUES ($1, $2, $3, $4) RETURNING *")
                .bind(ProjectId::new())
                .bind(slug.as_str())
                .bind(name)
                .bind(parent)
                .fetch_one(self.conn())
                .await?,
        )
    }

    pub async fn project(&mut self, id: ProjectId) -> Result<Option<Project>> {
        Ok(sqlx::query_as("SELECT * FROM projects WHERE id = $1")
            .bind(id)
            .fetch_optional(self.conn())
            .await?)
    }

    pub async fn project_by_slug(&mut self, slug: &str) -> Result<Option<Project>> {
        Ok(sqlx::query_as("SELECT * FROM projects WHERE slug = $1")
            .bind(slug)
            .fetch_optional(self.conn())
            .await?)
    }

    pub async fn projects(&mut self) -> Result<Vec<Project>> {
        Ok(sqlx::query_as("SELECT * FROM projects ORDER BY slug")
            .fetch_all(self.conn())
            .await?)
    }

    /// `project` followed by its ancestors, child first.
    pub async fn project_chain(&mut self, project: ProjectId) -> Result<Vec<Project>> {
        Ok(sqlx::query_as(
            "WITH RECURSIVE chain AS (
                 SELECT p.*, 0 AS depth FROM projects p WHERE p.id = $1
                 UNION ALL
                 SELECT p.*, c.depth + 1 FROM projects p JOIN chain c ON p.id = c.parent_id
                 WHERE c.depth < 64
             )
             SELECT id, slug, name, parent_id, settings, created_at FROM chain ORDER BY depth",
        )
        .bind(project)
        .fetch_all(self.conn())
        .await?)
    }

    /// Changes a project's parent. Rejects parents that would create a cycle.
    pub async fn set_project_parent(&mut self, project: ProjectId, parent: Option<ProjectId>) -> Result<()> {
        if let Some(parent) = parent
            && self.project_chain(parent).await?.iter().any(|p| p.id == project)
        {
            return Err(DbError::Invalid("parent would create a cycle".into()));
        }
        sqlx::query("UPDATE projects SET parent_id = $2 WHERE id = $1")
            .bind(project)
            .bind(parent)
            .execute(self.conn())
            .await?;
        Ok(())
    }

    pub async fn set_project_settings(&mut self, project: ProjectId, settings: &serde_json::Value) -> Result<()> {
        sqlx::query("UPDATE projects SET settings = $2 WHERE id = $1")
            .bind(project)
            .bind(settings)
            .execute(self.conn())
            .await?;
        Ok(())
    }

    pub async fn set_membership(&mut self, user: UserId, role: Role) -> Result<()> {
        sqlx::query(
            "INSERT INTO memberships (user_id, role) VALUES ($1, $2)
             ON CONFLICT (tenant_id, user_id) DO UPDATE SET role = EXCLUDED.role",
        )
        .bind(user)
        .bind(role)
        .execute(self.conn())
        .await?;
        Ok(())
    }

    pub async fn remove_membership(&mut self, user: UserId) -> Result<()> {
        sqlx::query("DELETE FROM memberships WHERE user_id = $1")
            .bind(user)
            .execute(self.conn())
            .await?;
        Ok(())
    }

    pub async fn role(&mut self, user: UserId) -> Result<Option<Role>> {
        Ok(sqlx::query_scalar("SELECT role FROM memberships WHERE user_id = $1")
            .bind(user)
            .fetch_optional(self.conn())
            .await?)
    }

    pub async fn members(&mut self) -> Result<Vec<Member>> {
        Ok(sqlx::query_as(
            "SELECT u.id AS user_id, u.email, u.name, m.role
             FROM memberships m JOIN users u ON u.id = m.user_id ORDER BY u.email",
        )
        .fetch_all(self.conn())
        .await?)
    }
}
