//! Mapping between Markdown files on disk and document variants.
//!
//! `dir/name.md` is the human variant of `dir/name`; `dir/name.ai.md` is its AI variant.
//! Also parses `git ls-files` output for source reports.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use solidate_app::core::sources::Files;
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

/// Path → blob id from `git ls-files -s -z` output (`<mode> <blob> <stage>\t<path>\0`).
pub fn git_files(out: &str) -> Result<Files> {
    out.split('\0')
        .filter(|e| !e.is_empty())
        .map(|e| {
            let (meta, path) = e
                .split_once('\t')
                .ok_or_else(|| anyhow!("unexpected git ls-files entry {e:?}"))?;
            let blob = meta
                .split(' ')
                .nth(1)
                .ok_or_else(|| anyhow!("unexpected git ls-files entry {e:?}"))?;
            Ok((path.to_owned(), blob.to_owned()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_git_ls_files() {
        let out = "100644 e69de29bb2d1d6434b8b29ae775ad8c2e48c5391 0\tREADME.md\x00100755 3b18e512dba79e4c8300dd08aeb37f8e728b8dad 0\tsrc/a b.rs\0";
        let f = git_files(out).unwrap();
        assert_eq!(f["README.md"], "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
        assert_eq!(f["src/a b.rs"], "3b18e512dba79e4c8300dd08aeb37f8e728b8dad");
        assert!(git_files("garbage\0").is_err());
    }

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
