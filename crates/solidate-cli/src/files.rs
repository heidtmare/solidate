//! Mapping between Markdown files on disk and document variants.
//!
//! `dir/name.md` is the human variant of `dir/name`; `dir/name.ai.md` is its AI variant.

use std::path::{Path, PathBuf};

use solidate_app::core::{DocPath, Variant};

/// Maps a file path relative to the import root. Returns `None` for non-Markdown
/// files and paths that are not valid document paths.
pub fn doc_for_file(rel: &Path) -> Option<(DocPath, Variant)> {
    let s = rel.to_str()?.replace('\\', "/");
    let stem = s.strip_suffix(".md")?;
    let (stem, variant) = match stem.strip_suffix(".ai") {
        Some(base) => (base, Variant::Ai),
        None => (stem, Variant::Human),
    };
    Some((DocPath::parse(stem).ok()?, variant))
}

pub fn file_for_doc(root: &Path, path: &DocPath, variant: Variant) -> PathBuf {
    let suffix = match variant {
        Variant::Human => ".md",
        Variant::Ai => ".ai.md",
    };
    root.join(format!("{path}{suffix}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapping_round_trips() {
        let (p, v) = doc_for_file(Path::new("design/auth.ai.md")).unwrap();
        assert_eq!((p.as_str(), v), ("design/auth", Variant::Ai));
        assert_eq!(doc_for_file(Path::new("readme.md")).unwrap().1, Variant::Human);
        assert!(doc_for_file(Path::new("image.png")).is_none());
        assert!(doc_for_file(Path::new("bad name.md")).is_none());
        assert_eq!(
            file_for_doc(Path::new("/out"), &p, v),
            Path::new("/out/design/auth.ai.md")
        );
    }
}
