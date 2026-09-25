//! Solidate domain logic. No I/O; storage, HTTP and MCP crates depend on this one.

pub mod auth;
pub mod diff;
pub mod hash;
pub mod include;
pub mod inherit;
pub mod links;
pub mod markdown;
pub mod path;
#[cfg(feature = "sqlx")]
mod sqlx_text;
pub mod sync;

pub use auth::{Role, Scope};
pub use hash::{Hash, Merkle};
pub use links::LinkTarget;
pub use markdown::{Analysis, Section, analyze, render_html};
pub use path::{DocPath, Slug};
pub use sync::{SyncBase, SyncState, Variant};
