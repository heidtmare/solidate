//! Request context and authorization.
//!
//! A [`Ctx`] separates who is acting ([`Principal`]) from what it may do
//! ([`Grant`]). Authentication schemes produce both; authorization reads only the
//! grant, and authorship (revisions, proposals, audit) reads only the principal.

use std::fmt;

use solidate_core::{Role, Scope};
use solidate_db::{Author, Project, ProjectId, Tenant, TokenId, UserId};

use crate::error::{AppError, Result};

/// The authenticated identity behind a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Principal {
    User(UserId),
    Token(TokenId),
    /// CLI and internal jobs.
    System,
}

impl fmt::Display for Principal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::User(id) => id.fmt(f),
            Self::Token(id) => id.fmt(f),
            Self::System => f.write_str("system"),
        }
    }
}

impl From<Principal> for Author {
    fn from(p: Principal) -> Self {
        match p {
            Principal::User(id) => Author::User(id),
            Principal::Token(id) => Author::Token(id),
            Principal::System => Author::System,
        }
    }
}

/// Permission level of a [`Grant`], in the form the credential carries it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Level {
    /// Tenant membership role.
    Role(Role),
    /// API token scopes.
    Scopes(Vec<Scope>),
    /// No restriction (CLI and internal jobs).
    Unrestricted,
}

/// What the caller may do within its tenant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub level: Level,
    /// When set, access is limited to this project (and reading its ancestors
    /// through inheritance).
    pub project: Option<ProjectId>,
}

impl Grant {
    pub fn unrestricted() -> Self {
        Self {
            level: Level::Unrestricted,
            project: None,
        }
    }

    pub fn allows(&self, access: Access, project: Option<ProjectId>) -> bool {
        let level_ok = match &self.level {
            Level::Unrestricted => true,
            Level::Role(role) => match access {
                Access::Read => true,
                Access::Write => role.can_write(),
                Access::Admin => role.can_admin(),
            },
            Level::Scopes(scopes) => {
                let required = match access {
                    Access::Read => Scope::Read,
                    Access::Write => Scope::Write,
                    Access::Admin => Scope::Admin,
                };
                scopes.iter().any(|s| s.allows(required))
            }
        };
        let project_ok = match self.project {
            None => true,
            Some(r) => project == Some(r),
        };
        level_ok && project_ok
    }
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
    pub principal: Principal,
    pub grant: Grant,
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
        self.grant.allows(access, project)
    }

    /// The project a restricted grant is bound to.
    pub fn restricted_project(&self) -> Option<ProjectId> {
        self.grant.project
    }

    pub fn author(&self) -> Author {
        self.principal.into()
    }

    pub fn user_id(&self) -> Option<UserId> {
        match self.principal {
            Principal::User(id) => Some(id),
            _ => None,
        }
    }
}
