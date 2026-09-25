//! Projects and inheritance.

use solidate_core::Slug;
use solidate_core::inherit::merge_settings;
use solidate_db::{Project, TenantTx};

use crate::App;
use crate::ctx::{Access, Ctx};
use crate::error::{AppError, Result, invalid};

impl App {
    pub async fn create_project(&self, ctx: &Ctx, slug: &Slug, name: &str, parent: Option<&str>) -> Result<Project> {
        ctx.require(Access::Admin, None)?;
        if name.trim().is_empty() {
            return Err(invalid("name is required"));
        }
        let mut tx = self.tx(ctx).await?;
        let parent = match parent {
            Some(s) => Some(project_by_slug(&mut tx, s).await?.id),
            None => None,
        };
        let p = tx.create_project(slug, name.trim(), parent).await?;
        tx.commit().await?;
        Ok(p)
    }

    /// Projects visible to `ctx`. Restricted tokens see their project only.
    pub async fn projects(&self, ctx: &Ctx) -> Result<Vec<Project>> {
        let mut projects = self.tx(ctx).await?.projects().await?;
        projects.retain(|p| ctx.allows(Access::Read, Some(p.id)));
        Ok(projects)
    }

    pub async fn project(&self, ctx: &Ctx, slug: &str) -> Result<Project> {
        let p = project_by_slug(&mut self.tx(ctx).await?, slug).await?;
        ctx.require(Access::Read, Some(&p))?;
        Ok(p)
    }

    /// `slug` followed by its ancestors.
    pub async fn project_chain(&self, ctx: &Ctx, slug: &str) -> Result<Vec<Project>> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, slug).await?;
        ctx.require(Access::Read, Some(&p))?;
        Ok(tx.project_chain(p.id).await?)
    }

    pub async fn set_project_parent(&self, ctx: &Ctx, slug: &str, parent: Option<&str>) -> Result<()> {
        ctx.require(Access::Admin, None)?;
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, slug).await?;
        let parent = match parent {
            Some(s) => Some(project_by_slug(&mut tx, s).await?.id),
            None => None,
        };
        tx.set_project_parent(p.id, parent).await?;
        Ok(tx.commit().await?)
    }

    pub async fn set_project_settings(&self, ctx: &Ctx, slug: &str, settings: &serde_json::Value) -> Result<()> {
        ctx.require(Access::Admin, None)?;
        if !settings.is_object() {
            return Err(invalid("settings must be a JSON object"));
        }
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, slug).await?;
        tx.set_project_settings(p.id, settings).await?;
        Ok(tx.commit().await?)
    }

    /// Settings merged from the root ancestor down to `slug`.
    pub async fn effective_settings(&self, ctx: &Ctx, slug: &str) -> Result<serde_json::Value> {
        let chain = self.project_chain(ctx, slug).await?;
        Ok(merge_settings(chain.iter().rev().map(|p| &p.settings)))
    }
}

pub(crate) async fn project_by_slug(tx: &mut TenantTx, slug: &str) -> Result<Project> {
    tx.project_by_slug(slug).await?.ok_or(AppError::NotFound)
}
