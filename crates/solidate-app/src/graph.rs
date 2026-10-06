//! Project-wide link graph (see [`solidate_core::graph`]).

use std::collections::HashMap;

use solidate_core::graph::{Graph, GraphDoc, build, references};
use solidate_core::{DocPath, Hash, Slug};
use solidate_db::Project;

use crate::App;
use crate::ctx::{Access, Ctx};
use crate::docs::effective_tree;
use crate::error::{AppError, Result};
use crate::projects::project_by_slug;

#[derive(Debug, Clone)]
pub struct LinkGraph {
    pub project: Project,
    /// Root hash of the project's tree; the graph changes only when it does.
    pub root_hash: Hash,
    pub graph: Graph,
}

impl App {
    /// Links and includes between the effective documents of `project`, from both
    /// variants.
    pub async fn link_graph(&self, ctx: &Ctx, project: &str) -> Result<LinkGraph> {
        let mut tx = self.tx(ctx).await?;
        let p = project_by_slug(&mut tx, project).await?;
        ctx.require(Access::Read, Some(&p))?;
        let chain = tx.project_chain(p.id).await?;
        let tree = effective_tree(&mut tx, p, &chain).await?;

        let mut contents = HashMap::new();
        for owner in &chain {
            for h in tx.head_contents(owner.id).await? {
                contents
                    .entry((owner.slug.clone(), h.path))
                    .or_insert_with(Vec::new)
                    .push((h.variant, h.content));
            }
        }
        let slug = |s: &str| Slug::parse(s).map_err(|e| AppError::Internal(e.to_string()));
        let docs = tree
            .entries
            .iter()
            .map(|e| {
                let path = DocPath::parse(&e.path).map_err(|e| AppError::Internal(e.to_string()))?;
                let refs = contents
                    .get(&(e.owner.clone(), e.path.clone()))
                    .into_iter()
                    .flatten()
                    .flat_map(|(variant, md)| references(md, &path, *variant))
                    .collect();
                Ok(GraphDoc {
                    path,
                    title: e.title.clone(),
                    owner: slug(&e.owner)?,
                    references: refs,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let chain = chain.iter().map(|c| slug(&c.slug)).collect::<Result<Vec<_>>>()?;
        Ok(LinkGraph {
            graph: build(&chain, &docs),
            project: tree.project,
            root_hash: tree.root_hash,
        })
    }
}
