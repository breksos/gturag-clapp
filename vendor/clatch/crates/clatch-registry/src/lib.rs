//! clatch-registry, the app library.
//!
//! Manifest (`clatch.json`), the registry record, and install / list / uninstall.
//! File-based, keyed by app id; Steam's library stripped to its essence
//! (reference/systems.md, reference/data-structures.md). KISS, no marketplace yet.

pub mod clapp;
pub mod fetch;
pub mod manifest;
pub mod record;
pub mod registry;

pub use manifest::{AgentSurface, CliCommand, Manifest, PerOs};
pub use record::{InstallState, RegistryRecord};
pub use registry::Registry;
