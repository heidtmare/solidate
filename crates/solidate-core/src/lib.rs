//! Solidate's pure domain logic. There's no I/O here: storage, HTTP and MCP live in
//! other crates and call into this one.

pub mod hash;
pub mod include;
pub mod inherit;
pub mod links;
pub mod markdown;
pub mod path;
pub mod sync;

pub use hash::{Hash, Merkle};
pub use links::LinkTarget;
pub use markdown::{Analysis, Section, analyze, render_html};
pub use path::{DocPath, Slug};
pub use sync::{SyncBase, SyncState, Variant};
