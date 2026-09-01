//! Stable, location-independent identities (reference/data-structures.md).
//!
//! Ids and names become path segments (`apps/<id>`, `agents/<name>.json`), so
//! every one that crosses a trust boundary (an installed manifest, an admin
//! request) must be [`AppId::valid`] / [`AgentName::valid`] before it is used as a
//! path. Construction stays infallible; validation is a gate the caller applies.

use crate::error::{ClatchError, Result};
use serde::{Deserialize, Serialize};
use std::fmt;

/// An app's reverse-DNS id, e.g. `com.arfium.arfchess`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AppId(String);

/// An agent's immutable identity, minted by the daemon at `agent.add`
/// (reference/daemon.md § Agents): the machine key everywhere - the engine
/// substrate dir, the daemon's maps, the timeline, the env identity
/// (`CLATCH_AGENT_ID`), and the app wire. Opaque here; the minting rule
/// (decimal unix seconds, monotonically bumped) lives with the daemon.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AgentId(String);

/// An agent's user-given handle: the natural-language pointer humans, the CLI,
/// and agent-to-agent speech use. Mutable (`agent.name.set`) but unique
/// (case-insensitively) at every moment, so it always resolves to exactly one
/// [`AgentId`]; never a storage or wire key (reference/daemon.md § Agents).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AgentName(String);

impl AppId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Reject anything unsafe as a path segment (traversal, separators, control).
    pub fn valid(&self) -> Result<()> {
        check("app id", &self.0)
    }
}

impl AgentId {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Reject anything unsafe as a path segment (traversal, separators, control).
    pub fn valid(&self) -> Result<()> {
        check("agent id", &self.0)
    }
}

impl AgentName {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Reject anything unsafe as a path segment (traversal, separators, control).
    pub fn valid(&self) -> Result<()> {
        check("agent name", &self.0)
    }
}

/// One safe-segment rule for ids, agent names, AND any other string that
/// becomes a path segment or a grant token (e.g. a clapp's `connector.cli`,
/// which becomes `bin/<cli>` and `Bash(<cli>:*)`): non-empty, bounded, only
/// `[A-Za-z0-9._-]`, and never `.` / `..` / a `..` substring. No separators,
/// so it cannot escape its dir, and no `*` / whitespace, so it cannot widen or
/// break a grant pattern. Public so every such surface enforces the SAME rule.
pub fn valid_segment(what: &str, s: &str) -> Result<()> {
    check(what, s)
}

fn check(what: &str, s: &str) -> Result<()> {
    let safe = !s.is_empty()
        && s.len() <= 128
        && s != "."
        && s != ".."
        && !s.contains("..")
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'));
    if safe {
        Ok(())
    } else {
        Err(ClatchError::Invalid(format!("{what} {s:?} is not allowed")))
    }
}

impl fmt::Display for AppId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for AgentId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal_and_separators() {
        for bad in [
            "",
            ".",
            "..",
            "../x",
            "a/b",
            "a\\b",
            "../../evil",
            "a..b",
            "x\0y",
        ] {
            assert!(AppId::new(bad).valid().is_err(), "should reject {bad:?}");
            assert!(
                AgentName::new(bad).valid().is_err(),
                "should reject {bad:?}"
            );
        }
    }

    #[test]
    fn accepts_ordinary_ids_and_names() {
        for ok in ["com.arfium.arfchess", "research", "my-agent_2", "a.b.c"] {
            assert!(AppId::new(ok).valid().is_ok(), "should accept {ok:?}");
            assert!(AgentName::new(ok).valid().is_ok(), "should accept {ok:?}");
        }
    }
}
