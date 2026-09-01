//! The registry record, Clatch's bookkeeping for one install
//! (reference/data-structures.md). Lives with the launcher, not in the content;
//! keyed by app id, never by path.

use crate::manifest::Manifest;
use chrono::{DateTime, Utc};
use clatch_core::{AppId, ElementType, SignalDecl, SignalType};
use serde::{Deserialize, Deserializer, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistryRecord {
    pub schema_version: u32,
    /// The element type (reference/elements.md). Defaults to `clapp`, so every
    /// pre-taxonomy record on disk keeps loading as what it was.
    #[serde(rename = "type", default)]
    pub element_type: ElementType,
    pub id: AppId,
    pub name: String,
    pub description: String,
    /// CLI shorthand; the agent self-documents via `<cli> --help`. `None` when the
    /// app declared no CLI (a signal-only or GUI-only app, reference/app-developer.md).
    pub cli: Option<String>,
    /// The CLI's declared verbs (name + one-liner), for the Library screen and
    /// per-command grants (reference/tools.md).
    #[serde(default)]
    pub commands: Vec<crate::manifest::CliCommand>,
    /// Declared signals with their manifest-fixed TYPES (reference/signals.md).
    /// Pre-Clapp-v1 records on disk carried bare name strings; those load as
    /// `context` (the fail-safe type: quiet, lossless, cannot trigger a turn or
    /// stage the chat buffer) until the app is reinstalled with a typed manifest.
    #[serde(deserialize_with = "signals_compat", default)]
    pub signals: Vec<SignalDecl>,
    /// Library presentation (reference/data-structures.md).
    #[serde(default)]
    pub about: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub version: String,
    /// Library banner path, relative to `install_dir`.
    #[serde(default)]
    pub banner: Option<String>,
    /// The app's icon path, relative to `install_dir` (the presentation asset for a
    /// desktop shortcut / a future library GUI). `None` if the app declared none.
    #[serde(default)]
    pub icon: Option<String>,
    pub install_dir: PathBuf,
    pub installed_at: DateTime<Utc>,
    /// The install source, the only seam the future marketplace touches
    /// (`local:<path>` now; `store:<pkg>@<ver>` later).
    pub source: String,
    /// A cli element's sign-in verb and probe verb (reference/elements.md
    /// § cli login); always `None` for other types (validate forbids them).
    #[serde(default)]
    pub login: Option<String>,
    #[serde(default)]
    pub login_check: Option<String>,
    /// The sign-out verb (reference/elements.md § cli login).
    #[serde(default)]
    pub logout: Option<String>,
    pub state: InstallState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    Installed,
}

/// One record-signal entry, either shape: the typed `{id, type}` or a bare
/// string. A record is Clatch's own cache of the manifest at install time, so an
/// old cache must keep loading (a bricked daemon over a stale cache fails the
/// fail-safe rule); the string form adopts `context`.
#[derive(Deserialize)]
#[serde(untagged)]
enum SignalCompat {
    Typed(SignalDecl),
    Legacy(String),
}

fn signals_compat<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<SignalDecl>, D::Error> {
    let raw = Vec::<SignalCompat>::deserialize(d)?;
    Ok(raw
        .into_iter()
        .map(|s| match s {
            SignalCompat::Typed(decl) => decl,
            SignalCompat::Legacy(id) => SignalDecl {
                id,
                signal_type: SignalType::Context,
            },
        })
        .collect())
}

impl RegistryRecord {
    pub fn from_manifest(
        m: &Manifest,
        install_dir: PathBuf,
        source: String,
        installed_at: DateTime<Utc>,
    ) -> Self {
        Self {
            schema_version: 1,
            element_type: m.element_type,
            id: m.id.clone(),
            name: m.name.clone(),
            description: m.description.clone(),
            cli: m.connector.cli.clone(),
            commands: m.connector.commands.clone(),
            signals: m.connector.signals.clone(),
            about: m.about.clone(),
            tags: m.tags.clone(),
            version: m.version.clone(),
            icon: m.icon.clone(),
            banner: m.banner.clone(),
            install_dir,
            installed_at,
            source,
            login: m.connector.login.clone(),
            login_check: m.connector.login_check.clone(),
            logout: m.connector.logout.clone(),
            state: InstallState::Installed,
        }
    }
}
