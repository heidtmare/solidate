//! Tracks sync between a document's human and AI variants.
//!
//! Each variant splits into sections, and sections are paired by anchor. For every
//! pair, Solidate records a [`SyncBase`]: the semantic hash each side had when the pair
//! was last reconciled. Comparing current hashes to that base tells which side moved:
//!
//! | human changed | AI changed | state         |
//! |---------------|------------|---------------|
//! | no            | no         | `InSync`      |
//! | yes           | no         | `HumanAhead`  |
//! | no            | yes        | `AiAhead`     |
//! | yes           | yes        | `Conflict`    |
//!
//! Additional rules:
//!
//! - A missing side counts as a hash of `None`. Adding or deleting a section is a
//!   change, so a section present only in the human variant is `HumanAhead` until
//!   its AI counterpart is written or the human section is deleted.
//! - If both sides currently have identical content, the pair is `InSync`.
//! - A pair that's missing on both sides is dropped from the plan.
//!
//! This module does not propagate content. External actors (users, agents) edit the
//! stale side; [`reconcile`] then records the new base.

use std::collections::{HashMap, HashSet};
use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

use crate::hash::Hash;
use crate::markdown::Section;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Human,
    Ai,
}

impl Variant {
    pub const ALL: [Self; 2] = [Self::Human, Self::Ai];

    pub fn other(self) -> Self {
        match self {
            Self::Human => Self::Ai,
            Self::Ai => Self::Human,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Human => "human",
            Self::Ai => "ai",
        }
    }
}

impl fmt::Display for Variant {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, thiserror::Error)]
#[error("unknown variant {0:?}: expected \"human\" or \"ai\"")]
pub struct ParseVariantError(String);

impl FromStr for Variant {
    type Err = ParseVariantError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "human" => Ok(Self::Human),
            "ai" => Ok(Self::Ai),
            other => Err(ParseVariantError(other.to_owned())),
        }
    }
}

/// Each side's semantic section hash at the last reconciliation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncBase {
    pub human: Option<Hash>,
    pub ai: Option<Hash>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncState {
    InSync,
    HumanAhead,
    AiAhead,
    Conflict,
}

impl SyncState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::InSync => "in_sync",
            Self::HumanAhead => "human_ahead",
            Self::AiAhead => "ai_ahead",
            Self::Conflict => "conflict",
        }
    }

    /// The variant that needs editing. `None` when the pair is in sync or in conflict.
    pub fn stale_side(self) -> Option<Variant> {
        match self {
            Self::HumanAhead => Some(Variant::Ai),
            Self::AiAhead => Some(Variant::Human),
            Self::InSync | Self::Conflict => None,
        }
    }

    pub fn needs_attention(self) -> bool {
        self != Self::InSync
    }
}

impl FromStr for SyncState {
    type Err = crate::auth::ParseEnumError;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        [Self::InSync, Self::HumanAhead, Self::AiAhead, Self::Conflict]
            .into_iter()
            .find(|v| v.as_str() == s)
            .ok_or_else(|| crate::auth::ParseEnumError::new("sync state", s))
    }
}

impl fmt::Display for SyncState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Classifies one section pair. Returns `None` if neither side exists.
pub fn classify(base: Option<&SyncBase>, human: Option<Hash>, ai: Option<Hash>) -> Option<SyncState> {
    if human.is_none() && ai.is_none() {
        return None;
    }
    if human == ai {
        return Some(SyncState::InSync);
    }
    let base = base.copied().unwrap_or_default();
    Some(match (human != base.human, ai != base.ai) {
        (false, false) => SyncState::InSync,
        (true, false) => SyncState::HumanAhead,
        (false, true) => SyncState::AiAhead,
        (true, true) => SyncState::Conflict,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionSync {
    pub anchor: String,
    pub state: SyncState,
    pub human: Option<Hash>,
    pub ai: Option<Hash>,
    pub base: Option<SyncBase>,
}

impl SectionSync {
    /// The base to record once this pair's current content is accepted as in sync.
    pub fn reconciled(&self) -> SyncBase {
        SyncBase {
            human: self.human,
            ai: self.ai,
        }
    }
}

/// `(anchor, hash)` pairs of analyzed sections, for [`plan`].
pub fn section_hashes(sections: &[Section]) -> Vec<(&str, Hash)> {
    sections.iter().map(|s| (s.anchor.as_str(), s.hash)).collect()
}

/// Classifies every section pair in a document. The result is ordered by the human
/// variant, then AI-only sections in AI order, then pairs left only in `bases`.
///
/// `human` and `ai` are `(anchor, semantic hash)` pairs in document order.
pub fn plan(human: &[(&str, Hash)], ai: &[(&str, Hash)], bases: &HashMap<String, SyncBase>) -> Vec<SectionSync> {
    let h: HashMap<&str, Hash> = human.iter().copied().collect();
    let a: HashMap<&str, Hash> = ai.iter().copied().collect();

    let mut order: Vec<&str> = Vec::new();
    let mut seen = HashSet::new();
    let mut base_keys: Vec<&str> = bases.keys().map(String::as_str).collect();
    base_keys.sort_unstable();
    let candidates = human
        .iter()
        .map(|(a, _)| *a)
        .chain(ai.iter().map(|(a, _)| *a))
        .chain(base_keys);
    for anchor in candidates {
        if seen.insert(anchor) {
            order.push(anchor);
        }
    }

    order
        .into_iter()
        .filter_map(|anchor| {
            let (hh, ah, base) = (h.get(anchor).copied(), a.get(anchor).copied(), bases.get(anchor));
            classify(base, hh, ah).map(|state| SectionSync {
                anchor: anchor.to_owned(),
                state,
                human: hh,
                ai: ah,
                base: base.copied(),
            })
        })
        .collect()
}

/// New bases after accepting the current content of `anchors` as in sync. Anchors
/// that are gone from both sides are dropped.
pub fn reconcile(plan: &[SectionSync], anchors: &[&str]) -> Vec<(String, SyncBase)> {
    plan.iter()
        .filter(|s| anchors.contains(&s.anchor.as_str()))
        .map(|s| (s.anchor.clone(), s.reconciled()))
        .collect()
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;
    use crate::markdown::analyze;

    fn h(s: &str) -> Option<Hash> {
        Some(Hash::of(s))
    }

    #[test]
    fn truth_table() {
        let base = SyncBase {
            human: h("h0"),
            ai: h("a0"),
        };
        let b = Some(&base);
        assert_eq!(classify(b, h("h0"), h("a0")), Some(SyncState::InSync));
        assert_eq!(classify(b, h("h1"), h("a0")), Some(SyncState::HumanAhead));
        assert_eq!(classify(b, h("h0"), h("a1")), Some(SyncState::AiAhead));
        assert_eq!(classify(b, h("h1"), h("a1")), Some(SyncState::Conflict));
        assert_eq!(classify(b, None, h("a0")), Some(SyncState::HumanAhead), "human deleted");
        assert_eq!(classify(b, None, None), None, "deleted on both sides");
        assert_eq!(classify(b, h("same"), h("same")), Some(SyncState::InSync));
        assert_eq!(classify(None, h("new"), None), Some(SyncState::HumanAhead));
        assert_eq!(classify(None, h("x"), h("y")), Some(SyncState::Conflict));
    }

    #[test]
    fn plan_over_documents() {
        let human = analyze("# A\n\nhuman a\n\n# B\n\nhuman b v2\n\n# New\n\nfresh\n", None);
        let ai = analyze("# A\n\nai a\n\n# B\n\nai b\n\n# Gone\n\nold\n", None);
        let bases = HashMap::from([
            (
                "a".into(),
                SyncBase {
                    human: Some(human.section("a").unwrap().hash),
                    ai: Some(ai.section("a").unwrap().hash),
                },
            ),
            (
                "b".into(),
                SyncBase {
                    human: h("stale"),
                    ai: Some(ai.section("b").unwrap().hash),
                },
            ),
            (
                "gone".into(),
                SyncBase {
                    human: h("old"),
                    ai: Some(ai.section("gone").unwrap().hash),
                },
            ),
        ]);
        let p = plan(&section_hashes(&human.sections), &section_hashes(&ai.sections), &bases);
        let got: Vec<_> = p.iter().map(|s| (s.anchor.as_str(), s.state)).collect();
        assert_eq!(
            got,
            [
                ("a", SyncState::InSync),
                ("b", SyncState::HumanAhead),
                ("new", SyncState::HumanAhead),
                ("gone", SyncState::HumanAhead),
            ]
        );
        let rec = reconcile(&p, &["b"]);
        assert_eq!(
            rec[0].1,
            SyncBase {
                human: Some(human.section("b").unwrap().hash),
                ai: Some(ai.section("b").unwrap().hash)
            }
        );
    }

    proptest! {
        /// After reconciling, a pair is always in sync until one side changes, and a
        /// change on one side always points at the other side as stale.
        #[test]
        fn reconcile_then_edit(h0 in any::<[u8; 4]>(), a0 in any::<[u8; 4]>(), h1 in any::<[u8; 4]>()) {
            let (h0, a0, h1) = (Hash::of(h0), Hash::of(a0), Hash::of(h1));
            let base = SyncBase { human: Some(h0), ai: Some(a0) };
            prop_assert_eq!(classify(Some(&base), Some(h0), Some(a0)), Some(SyncState::InSync));
            if h1 != h0 && h1 != a0 {
                let s = classify(Some(&base), Some(h1), Some(a0)).unwrap();
                prop_assert_eq!(s.stale_side(), Some(Variant::Ai));
            }
        }
    }
}
