//! Multi-project inheritance.
//!
//! Projects form a tree. A document path resolves to the nearest project in the
//! chain, from the child up to the root, that defines it, so a child overrides a
//! document by defining the same path. Settings (branding, rules) merge down the
//! chain, and child values win.

use serde_json::Value;

/// Returns the first `Some` from `lookup`, walking `chain` from child to root, along
/// with the chain index it came from (0 means the child's own).
pub fn resolve_first<P, T>(chain: &[P], mut lookup: impl FnMut(&P) -> Option<T>) -> Option<(usize, T)> {
    chain.iter().enumerate().find_map(|(i, p)| lookup(p).map(|t| (i, t)))
}

/// Deep-merges JSON settings from root to child. Objects merge key by key. Any other
/// value, arrays included, is replaced by the child's. `null` in a child deletes the key.
pub fn merge_settings<'a>(root_to_child: impl IntoIterator<Item = &'a Value>) -> Value {
    let mut acc = Value::Object(Default::default());
    for layer in root_to_child {
        merge_into(&mut acc, layer);
    }
    acc
}

fn merge_into(acc: &mut Value, layer: &Value) {
    match (acc, layer) {
        (Value::Object(a), Value::Object(l)) => {
            for (k, v) in l {
                if v.is_null() {
                    a.remove(k);
                } else {
                    merge_into(a.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        (acc, layer) => *acc = layer.clone(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn nearest_definition_wins() {
        let chain = ["child", "parent", "root"];
        let found = resolve_first(&chain, |p| (*p != "child").then(|| format!("from {p}")));
        assert_eq!(found, Some((1, "from parent".to_owned())));
    }

    #[test]
    fn settings_merge_down() {
        let root = json!({"brand": {"color": "blue", "logo": "r.svg"}, "rules": ["a"], "x": 1});
        let child = json!({"brand": {"color": "red"}, "rules": ["b"], "x": null});
        assert_eq!(
            merge_settings([&root, &child]),
            json!({"brand": {"color": "red", "logo": "r.svg"}, "rules": ["b"]})
        );
    }
}
