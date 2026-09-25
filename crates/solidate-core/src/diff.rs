//! Line diffs.

use similar::Algorithm;
use similar::udiff::unified_diff;

/// Unified diff of `old` → `new` with 3 lines of context. Empty when equal.
pub fn unified(old: &str, new: &str, old_label: &str, new_label: &str) -> String {
    if old == new {
        return String::new();
    }
    unified_diff(Algorithm::Patience, old, new, 3, Some((old_label, new_label)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_marks_changed_lines() {
        let d = unified("a\nb\nc\n", "a\nB\nc\n", "old", "new");
        assert!(d.contains("--- old") && d.contains("-b") && d.contains("+B"), "{d}");
        assert!(unified("x\n", "x\n", "a", "b").is_empty());
    }
}
