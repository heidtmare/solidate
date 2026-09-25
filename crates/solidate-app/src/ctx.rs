//! Request context and authorization.

use solidate_core::{Role, Scope};
use solidate_db::{Author, Project, ProjectId, Tenant, TokenId, UserId};

use crate::error::{AppError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Actor {
    User {
        id: UserId,
        role: Role,
    },
    /// `project`: when set, the token may access only that project (and read its
    /// ancestors through inheritance).
    Token {
        id: TokenId,
        scopes: Vec<Scope>,
        project: Option<ProjectId>,
    },
    /// CLI and internal jobs. Unrestricted.
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Access {
    Read,
    Write,
    Admin,
}

/// Authenticated caller within one tenant.
#[derive(Debug, Clone)]
pub struct Ctx {
    pub tenant: Tenant,
    pub actor: Actor,
}

impl Ctx {
    /// Checks `access` on `project`, or tenant-wide when `project` is `None`.
    pub fn require(&self, access: Access, project: Option<&Project>) -> Result<()> {
        if self.allows(access, project.map(|p| p.id)) {
            Ok(())
        } else {
            Err(AppError::Forbidden)
        }
    }

    pub fn allows(&self, access: Access, project: Option<ProjectId>) -> bool {
        match &self.actor {
            Actor::System => true,
            Actor::User { role, .. } => match access {
                Access::Read => true,
                Access::Write => role.can_write(),
                Access::Admin => role.can_admin(),
            },
            Actor::Token {
                scopes,
                project: restricted,
                ..
            } => {
                let scope = match access {
                    Access::Read => Scope::Read,
                    Access::Write => Scope::Write,
                    Access::Admin => Scope::Admin,
                };
                let project_ok = match restricted {
                    None => true,
                    Some(r) => project == Some(*r),
                };
                project_ok && scopes.iter().any(|s| s.allows(scope))
            }
        }
    }

    /// The project a restricted token is bound to.
    pub fn restricted_project(&self) -> Option<ProjectId> {
        match &self.actor {
            Actor::Token { project, .. } => *project,
            _ => None,
        }
    }

    pub fn author(&self) -> Author {
        match &self.actor {
            Actor::User { id, .. } => Author::User(*id),
            Actor::Token { id, .. } => Author::Token(*id),
            Actor::System => Author::System,
        }
    }

    pub fn user_id(&self) -> Option<UserId> {
        match &self.actor {
            Actor::User { id, .. } => Some(*id),
            _ => None,
        }
    }
}
