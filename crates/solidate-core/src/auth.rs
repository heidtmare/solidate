//! Membership roles and API token scopes.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// A user's role within a tenant. Ordered by privilege.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Reader,
    Editor,
    Admin,
}

/// Permission carried by an API token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    Read,
    Write,
    Admin,
}

#[derive(Debug, thiserror::Error)]
#[error("unknown {kind} {value:?}")]
pub struct ParseEnumError {
    kind: &'static str,
    value: String,
}

impl ParseEnumError {
    pub(crate) fn new(kind: &'static str, value: &str) -> Self {
        Self {
            kind,
            value: value.to_owned(),
        }
    }
}

macro_rules! text_enum {
    ($ty:ident, $kind:literal, { $($variant:ident => $s:literal),* $(,)? }) => {
        impl $ty {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $s),* }
            }
        }

        impl fmt::Display for $ty {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $ty {
            type Err = ParseEnumError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                match s {
                    $($s => Ok(Self::$variant),)*
                    other => Err(ParseEnumError { kind: $kind, value: other.to_owned() }),
                }
            }
        }
    };
}

text_enum!(Role, "role", { Reader => "reader", Editor => "editor", Admin => "admin" });
text_enum!(Scope, "scope", { Read => "read", Write => "write", Admin => "admin" });

impl Role {
    pub fn can_write(self) -> bool {
        self >= Self::Editor
    }

    pub fn can_admin(self) -> bool {
        self == Self::Admin
    }
}

impl Scope {
    /// Whether a token holding `self` satisfies `required`. Scopes are hierarchical.
    pub fn allows(self, required: Scope) -> bool {
        self >= required
    }
}
