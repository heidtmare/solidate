use solidate_db::SearchHit;

use crate::App;
use crate::ctx::{Access, Ctx};
use crate::error::{AppError, Result, invalid};
use crate::projects::project_by_slug;

impl App {
    /// Full-text search. Restricted tokens are limited to their project.
    pub async fn search(&self, ctx: &Ctx, query: &str, project: Option<&str>, limit: i64) -> Result<Vec<SearchHit>> {
        let query = query.trim();
        if query.is_empty() {
            return Err(invalid("query is empty"));
        }
        let mut tx = self.tx(ctx).await?;
        let project_id = match project {
            Some(slug) => {
                let p = project_by_slug(&mut tx, slug).await?;
                ctx.require(Access::Read, Some(&p))?;
                Some(p.id)
            }
            None => {
                let restricted = ctx.restricted_project();
                if !ctx.allows(Access::Read, restricted) {
                    return Err(AppError::Forbidden);
                }
                restricted
            }
        };
        Ok(tx.search(query, project_id, limit.clamp(1, 100)).await?)
    }
}
