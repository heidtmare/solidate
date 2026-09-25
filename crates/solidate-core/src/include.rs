//! Transclusion: expands `{{include [project:]path[#anchor]}}` directives.
//!
//! Expansion is recursive, with cycle detection and a depth limit. Each included
//! piece is recorded with its hash; [`Expansion::resolved_hash`] folds these into the
//! including document's hash, so upstream changes invalidate dependents.

use comrak::{Arena, parse_document};

use crate::hash::{Hash, Merkle};
use crate::links::LinkTarget;
use crate::markdown::{analyze, options, top_level_includes};
use crate::path::Slug;

pub const DEFAULT_MAX_DEPTH: usize = 8;

/// Looks up the full source of an include target. The target is fully qualified:
/// `project` is always set.
pub trait IncludeResolver {
    fn resolve(&mut self, target: &LinkTarget) -> Option<String>;
}

impl<F: FnMut(&LinkTarget) -> Option<String>> IncludeResolver for F {
    fn resolve(&mut self, target: &LinkTarget) -> Option<String> {
        self(target)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dependency {
    pub target: LinkTarget,
    /// Hash of the included text, or `None` if the target couldn't be resolved.
    pub hash: Option<Hash>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expansion {
    pub text: String,
    /// Every include encountered, including nested ones, in expansion order.
    pub dependencies: Vec<Dependency>,
}

impl Expansion {
    /// Combines the source's content hash with every dependency's hash.
    pub fn resolved_hash(&self, source: Hash) -> Hash {
        if self.dependencies.is_empty() {
            return source;
        }
        let mut m = Merkle::new();
        m.insert("", source);
        for (i, d) in self.dependencies.iter().enumerate() {
            m.insert(format!("{i}:{}", d.target), d.hash.unwrap_or(Hash::of("")));
        }
        m.finish()
    }
}

/// Expands includes in `md`. `project` is the slug of the including document's
/// project, which unqualified targets resolve against.
pub fn expand_includes(md: &str, project: &Slug, resolver: &mut dyn IncludeResolver, max_depth: usize) -> Expansion {
    let mut deps = Vec::new();
    let mut stack = Vec::new();
    let text = expand(md, project, resolver, max_depth, &mut stack, &mut deps);
    Expansion {
        text,
        dependencies: deps,
    }
}

fn expand(
    md: &str,
    project: &Slug,
    resolver: &mut dyn IncludeResolver,
    depth_left: usize,
    stack: &mut Vec<String>,
    deps: &mut Vec<Dependency>,
) -> String {
    let arena = Arena::new();
    let root = parse_document(&arena, md, &options());
    let includes = top_level_includes(root);
    if includes.is_empty() {
        return md.to_owned();
    }

    let lines: Vec<&str> = md.split_inclusive('\n').collect();
    let mut out = String::with_capacity(md.len());
    let mut next_line = 1;
    for (target, (start, end)) in includes {
        out.push_str(&lines[next_line - 1..start - 1].concat());
        next_line = end + 1;

        let target = LinkTarget {
            project: Some(target.project.clone().unwrap_or_else(|| project.clone())),
            ..target
        };
        let key = target.to_string();
        let replacement = if stack.contains(&key) {
            deps.push(Dependency { target, hash: None });
            notice("Include cycle", &key)
        } else if depth_left == 0 {
            deps.push(Dependency { target, hash: None });
            notice("Include depth exceeded", &key)
        } else {
            match resolver
                .resolve(&target)
                .and_then(|src| select(&src, target.anchor.as_deref()))
            {
                None => {
                    deps.push(Dependency { target, hash: None });
                    notice("Missing include", &key)
                }
                Some(piece) => {
                    deps.push(Dependency {
                        target: target.clone(),
                        hash: Some(Hash::of(&piece)),
                    });
                    let inner_project = target.project.clone().expect("qualified above");
                    stack.push(key);
                    let expanded = expand(&piece, &inner_project, resolver, depth_left - 1, stack, deps);
                    stack.pop();
                    expanded
                }
            }
        };
        out.push_str(&replacement);
        if !out.ends_with('\n') {
            out.push('\n');
        }
    }
    out.push_str(&lines[(next_line - 1).min(lines.len())..].concat());
    out
}

fn select(src: &str, anchor: Option<&str>) -> Option<String> {
    match anchor {
        None => Some(src.to_owned()),
        Some(a) => analyze(src, None).section_tree_source(a),
    }
}

fn notice(label: &str, key: &str) -> String {
    format!("> **{label}:** `{key}`\n")
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn slug(s: &str) -> Slug {
        Slug::parse(s).unwrap()
    }

    fn resolver(docs: &[(&str, &str)]) -> impl FnMut(&LinkTarget) -> Option<String> {
        let docs: HashMap<String, String> = docs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        move |t: &LinkTarget| {
            let key = format!("{}:{}", t.project.as_ref().unwrap(), t.path.as_ref().unwrap());
            docs.get(&key).cloned()
        }
    }

    #[test]
    fn expands_whole_docs_sections_and_nested() {
        let mut r = resolver(&[
            (
                "shared:rules",
                "# Rules\n\n## Tone {#tone}\n\nBe kind.\n\n### Detail\n\nReally.\n\n## Other\n\nNope.\n",
            ),
            ("app:a", "A says hi.\n\n{{include b}}\n"),
            ("app:b", "B here.\n"),
        ]);
        let md = "Top\n\n{{include shared:rules#tone}}\n\n{{include a}}\n\nEnd\n";
        let e = expand_includes(md, &slug("app"), &mut r, DEFAULT_MAX_DEPTH);
        assert!(e.text.contains("Be kind.") && e.text.contains("Really."));
        assert!(!e.text.contains("Nope."));
        assert!(e.text.contains("A says hi.") && e.text.contains("B here."));
        assert!(e.text.starts_with("Top\n") && e.text.ends_with("End\n"));
        let keys: Vec<_> = e.dependencies.iter().map(|d| d.target.to_string()).collect();
        assert_eq!(keys, ["shared:rules#tone", "app:a", "app:b"]);
    }

    #[test]
    fn detects_cycles_and_missing() {
        let mut r = resolver(&[("app:a", "{{include b}}\n"), ("app:b", "{{include a}}\n")]);
        let e = expand_includes(
            "{{include a}}\n\n{{include nope}}\n",
            &slug("app"),
            &mut r,
            DEFAULT_MAX_DEPTH,
        );
        assert!(e.text.contains("Include cycle"), "{}", e.text);
        assert!(e.text.contains("Missing include:** `app:nope`"));
    }

    #[test]
    fn resolved_hash_tracks_dependencies() {
        let md = "{{include shared:x}}\n";
        let h = |content: &str| {
            let docs = [("shared:x", content)];
            let mut r = resolver(&docs);
            expand_includes(md, &slug("app"), &mut r, 4).resolved_hash(Hash::of(md))
        };
        assert_ne!(h("one"), h("two"));
        assert_eq!(h("one"), h("one"));
    }
}
