//! The element taxonomy (reference/elements.md): what a package IS. The type
//! is declared in the manifest, never inferred, and each type carries a locked
//! set of required and FORBIDDEN surfaces the validator enforces.

use serde::{Deserialize, Serialize};

/// The three element types. `Clapp` is the serde default so every
/// pre-taxonomy manifest and registry record stays valid unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ElementType {
    /// The full Clatch app: launch + control pipe + CLI (+ signals).
    #[default]
    Clapp,
    /// A packaged CLI tool: no process, no pipe, NO app->agent path, ever.
    Cli,
    /// An instruction package (`SKILL.md`) the engine loads for an agent.
    Skill,
}

impl ElementType {
    pub fn as_str(self) -> &'static str {
        match self {
            ElementType::Clapp => "clapp",
            ElementType::Cli => "cli",
            ElementType::Skill => "skill",
        }
    }
}

impl std::fmt::Display for ElementType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}
