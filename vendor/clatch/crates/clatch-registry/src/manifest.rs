//! `clatch.json`, the app↔launcher contract (reference/data-structures.md).
//! Clatch reads only: identity, how to launch, and the agent surface
//! (cli shorthand + signals). Nothing about the app's internals.

use clatch_core::{AppId, ClatchError, ElementType, Result, SUPPORTED_PROTOCOL};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub manifest_version: u32,
    /// The element type (reference/elements.md): declared, never inferred.
    /// Defaults to `clapp` so every pre-taxonomy manifest stays valid; the
    /// per-type required/forbidden matrix is enforced in [`Self::validate`].
    #[serde(rename = "type", default)]
    pub element_type: ElementType,
    pub id: AppId,
    pub name: String,
    pub description: String,
    pub version: String,
    /// The control-pipe major. Required (and version-checked) for a clapp;
    /// FORBIDDEN for cli/skill, which speak no pipe - serde-defaults to 0 so
    /// a pipe-less manifest parses, then the matrix decides.
    #[serde(default)]
    pub protocol: u32,
    #[serde(default)]
    pub icon: Option<String>,
    /// Library banner image (relative to the content root), the app page header.
    #[serde(default)]
    pub banner: Option<String>,
    /// Long-form Library text (`description` stays the one-liner).
    #[serde(default)]
    pub about: Option<String>,
    /// Library tags, e.g. `["game", "chess"]`.
    #[serde(default)]
    pub tags: Vec<String>,
    /// The process surface: required for a clapp, forbidden for cli/skill
    /// (the matrix in [`Self::validate`]). Serde-defaults to empty so a
    /// launch-less manifest PARSES, then the matrix decides.
    #[serde(default)]
    pub launch: PerOs,
    /// The agent-facing surface. The CLI is its constant (validate enforces
    /// it, reference/tools.md § Connectors); signals may be empty (an optional
    /// facet never forks the class). The block still serde-defaults so a
    /// missing one PARSES and then fails validate with the precise error.
    #[serde(default)]
    pub connector: AgentSurface,
}

/// One CLI verb, machine-readable (reference/data-structures.md): the Library
/// screen shows it and grants can target it (`Bash(<cli> <name>:*)`). NOT the
/// agent's manual; `--help` stays that.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CliCommand {
    pub name: String,
    #[serde(default)]
    pub about: String,
}

/// Per-OS launch command, relative to the content root.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerOs {
    #[serde(default)]
    pub linux: Option<String>,
    #[serde(default)]
    pub windows: Option<String>,
    #[serde(default)]
    pub macos: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
}

impl PerOs {
    /// The launch command for the host OS, if declared. No silent cross-OS
    /// fallback: a missing command is a misconfigured manifest for this platform.
    pub fn command(&self) -> Option<&str> {
        if cfg!(target_os = "windows") {
            self.windows.as_deref()
        } else if cfg!(target_os = "macos") {
            self.macos.as_deref()
        } else {
            self.linux.as_deref()
        }
    }

    /// The host-OS launch command resolved against the content `root`: an
    /// absolute command as-is, else relative to the root. The ONE place this
    /// rule lives, so the installer, the validator, and the spawner (which
    /// actually execs it) can never resolve the same manifest to different paths.
    pub fn resolve(&self, root: &Path) -> Option<PathBuf> {
        self.command().map(|cmd| {
            if Path::new(cmd).is_absolute() {
                PathBuf::from(cmd)
            } else {
                root.join(cmd)
            }
        })
    }
}

/// The face Clatch exposes to the agent: the CLI shorthand (the clapp's
/// constant, self-documented via `--help`; `-h` is the floor), the CLI
/// binary's path, and the declared signal vocabulary (may be empty). No
/// manual file.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSurface {
    #[serde(default)]
    pub cli: Option<String>,
    /// The CLI binary, relative to the content root; defaults to `bin/<cli>`.
    /// Clatch links it onto the agent's PATH (reference/tools.md).
    #[serde(default)]
    pub cli_bin: Option<String>,
    /// The CLI's verb list (flat, v1), for the Library screen and per-command
    /// grants (reference/tools.md).
    #[serde(default)]
    pub commands: Vec<CliCommand>,
    #[serde(default)]
    pub signals: Vec<clatch_core::SignalDecl>,
    /// A cli element's sign-in verb, run as `<cli> <login>` (a hidden child;
    /// the browser is the visible surface, reference/elements.md § cli login).
    /// Forbidden on clapp (its GUI owns auth) and skill.
    #[serde(default)]
    pub login: Option<String>,
    /// The probe verb: exit 0 = signed in. Absent = the state is unknown and
    /// never claimed. Same per-type rule as `login`.
    #[serde(default)]
    pub login_check: Option<String>,
    /// The sign-OUT verb. Signing in is half a contract without it
    /// (reference/elements.md, 2026-08-01): a tool that can take a credential
    /// must be able to give it back, and `uninstall --purge` is a DIFFERENT
    /// act - it erases Clatch's copy while the vendor may still hold a live
    /// session only its own verb can end. Same per-type rule as `login`.
    #[serde(default)]
    pub logout: Option<String>,
}

impl AgentSurface {
    /// The CLI binary's path relative to the content root: the explicit `cliBin`
    /// override, else the `bin/<cli>` convention. `None` if no CLI is declared.
    pub fn cli_bin(&self) -> Option<String> {
        let cli = self.cli.as_deref()?;
        Some(self.cli_bin.clone().unwrap_or_else(|| format!("bin/{cli}")))
    }

    /// That binary INSIDE `root`, resolved the way an OS resolves an
    /// executable: the declared path, else the same path carrying one of the
    /// host's executable extensions.
    ///
    /// `cli` is a NAME, not a filename. A cross-platform element ships
    /// `bin/parts` and `bin/parts.exe` side by side under ONE manifest, and
    /// requiring the author to spell `.exe` would force either a per-OS
    /// `cliBin` map (which is what `launch` is, and the CLI does not need that
    /// shape) or a Windows-only package. Before this, such a package failed
    /// `validate` on Windows and INSTALLED there with no CLI at all: the link
    /// step skips a target it cannot find, so the shorthand the agent was
    /// granted silently did not exist.
    pub fn cli_bin_in(&self, root: &Path) -> Option<PathBuf> {
        resolve_exe(root.join(self.cli_bin()?), CLI_EXTENSIONS)
    }
}

/// `declared`, else the first `<declared>.<ext>` that exists. Extensions are
/// APPENDED, never substituted, which is what `PATHEXT` itself does: a declared
/// `bin/tool.js` looks for `bin/tool.js.exe`, not `bin/tool.exe`.
///
/// Split out from the manifest so the Windows answer is exercised on the
/// machines that WRITE it: the extension list is the only per-OS part.
fn resolve_exe(declared: PathBuf, exts: &[&str]) -> Option<PathBuf> {
    if declared.exists() {
        return Some(declared);
    }
    exts.iter()
        .map(|ext| PathBuf::from(format!("{}.{ext}", declared.display())))
        .find(|p| p.exists())
}

/// The extensions a shipped CLI binary may carry on the host. Windows needs
/// them (a bare name resolves through `PATHEXT`); unix executables carry none,
/// so the declared path is the only answer there.
const CLI_EXTENSIONS: &[&str] = if cfg!(windows) {
    &["exe", "cmd", "bat"]
} else {
    &[]
};

impl Manifest {
    pub fn parse(json: &str) -> Result<Self> {
        serde_json::from_str(json).map_err(|e| ClatchError::Invalid(format!("clatch.json: {e}")))
    }

    /// Reject a manifest that crosses its element type's contract
    /// (reference/elements.md: the locked required/forbidden matrix). Forbidden
    /// surfaces are REJECTED, never ignored - a silent drop would let a package
    /// believe it declared something it never got.
    pub fn validate(&self) -> Result<()> {
        if self.manifest_version != 1 {
            return Err(ClatchError::Invalid(format!(
                "manifestVersion {} unsupported (expected 1)",
                self.manifest_version
            )));
        }
        let ty = self.element_type;
        let bad = |what: &str| {
            Err(ClatchError::Invalid(format!(
                "clatch.json ({ty} element): {what}"
            )))
        };
        // The id becomes a path segment (apps/<id>); reject traversal/separators.
        self.id.valid()?;
        if self.id.as_str().is_empty() {
            return bad("id is empty");
        }
        if self.name.is_empty() {
            return bad("name is empty");
        }
        if self.description.is_empty() {
            return bad("description is empty");
        }
        if self.version.is_empty() {
            return bad("version is empty");
        }

        // The pipe major: a clapp must speak one this launcher speaks (then a
        // running instance is compatible by construction, protocol.md
        // § Versioning); a cli/skill has no pipe, so declaring one is the
        // matrix's forbidden-surface error, not an ignored field.
        match ty {
            ElementType::Clapp => {
                if self.protocol != SUPPORTED_PROTOCOL {
                    return Err(ClatchError::Invalid(format!(
                        "protocol {} unsupported (launcher supports up to {SUPPORTED_PROTOCOL})",
                        self.protocol
                    )));
                }
            }
            ElementType::Cli | ElementType::Skill => {
                if self.protocol != 0 {
                    return bad("`protocol` is forbidden (this type speaks no control pipe)");
                }
            }
        }

        // launch: the clapp's process surface; forbidden where no process exists.
        let has_launch = self.launch.linux.is_some()
            || self.launch.windows.is_some()
            || self.launch.macos.is_some();
        match ty {
            ElementType::Clapp => {
                if !has_launch {
                    return bad("launch has no per-OS command");
                }
            }
            ElementType::Cli | ElementType::Skill => {
                if has_launch || !self.launch.args.is_empty() {
                    return bad("`launch` is forbidden (this type has no process)");
                }
            }
        }

        // The CLI surface: the constant of clapp AND cli (`-h` is the floor);
        // forbidden on skill (knowledge, not commands).
        match ty {
            ElementType::Clapp | ElementType::Cli => {
                match self.connector.cli.as_deref() {
                    None => {
                        return bad("connector.cli is missing (this type always ships its CLI; `-h` is the floor)")
                    }
                    Some("") => return bad("connector.cli is present but empty"),
                    // The cli becomes a path segment (`bin/<cli>`, linked into
                    // `~/.clatch/bin`) AND a grant token (`Bash(<cli>:*)`), so a
                    // third-party package must not smuggle traversal (`../`),
                    // separators, `*`, or whitespace through it (the same
                    // safe-segment rule the id rides).
                    Some(cli) => clatch_core::valid_segment("connector.cli", cli)?,
                }
                // Declared commands become permission patterns; empty or
                // duplicate names would make grants ambiguous.
                if self
                    .connector
                    .commands
                    .iter()
                    .any(|c| c.name.trim().is_empty())
                {
                    return bad("connector.commands has an empty name");
                }
                let mut names: Vec<&str> = self
                    .connector
                    .commands
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect();
                names.sort_unstable();
                if names.windows(2).any(|w| w[0] == w[1]) {
                    return bad("connector.commands has a duplicate name");
                }
            }
            ElementType::Skill => {
                if self.connector.cli.is_some()
                    || self.connector.cli_bin.is_some()
                    || !self.connector.commands.is_empty()
                {
                    return bad(
                        "`connector` CLI surfaces are forbidden (a skill informs, it does not run)",
                    );
                }
            }
        }

        // Signals: a clapp's declared vocabulary; FORBIDDEN on cli (a cli
        // carries no app->agent path, EVER - the definition of the type,
        // reference/elements.md) and on skill.
        match ty {
            ElementType::Clapp => {
                if self.connector.signals.iter().any(|s| s.id.is_empty()) {
                    return bad("connector.signals has an empty id");
                }
            }
            ElementType::Cli | ElementType::Skill => {
                if !self.connector.signals.is_empty() {
                    return bad(
                        "`connector.signals` is forbidden (this type has no app->agent path)",
                    );
                }
            }
        }

        // Login verbs: cli only (a clapp's GUI owns its auth; a skill has
        // nothing to sign into).
        if ty != ElementType::Cli
            && (self.connector.login.is_some()
                || self.connector.login_check.is_some()
                || self.connector.logout.is_some())
        {
            return bad("`connector.login`/`loginCheck`/`logout` are cli-element surfaces");
        }
        if self.connector.login.as_deref() == Some("") {
            return bad("connector.login is present but empty");
        }
        if self.connector.login_check.as_deref() == Some("") {
            return bad("connector.loginCheck is present but empty");
        }
        if self.connector.logout.as_deref() == Some("") {
            return bad("connector.logout is present but empty");
        }
        Ok(())
    }

    /// The files a well-formed package must ship, checked against its content
    /// `root`, BY TYPE (reference/elements.md): a clapp's host-OS launch
    /// binary, a clapp/cli's declared CLI binary, a skill's `SKILL.md`, and
    /// everyone's declared icon. The ONE gate for "this package is complete on
    /// disk", so `install` and `clatch validate` can never disagree about
    /// whether a package is good (they once did: install skipped the
    /// no-host-OS case that validate rejected). Every future tightening lands
    /// here and both inherit it.
    pub fn check_files(&self, root: &Path) -> Result<()> {
        if self.element_type == ElementType::Clapp {
            let bin = self.launch.resolve(root).ok_or_else(|| {
                ClatchError::Invalid(format!(
                    "no launch command for this OS ({})",
                    std::env::consts::OS
                ))
            })?;
            if !bin.exists() {
                return Err(ClatchError::Invalid(format!(
                    "launch command not found in the package: {} (broken build?)",
                    bin.display()
                )));
            }
        }
        if self.element_type == ElementType::Skill && !root.join("SKILL.md").exists() {
            return Err(ClatchError::Invalid(
                "SKILL.md not found at the package root (a skill's whole voice; \
                 reference/elements.md)"
                    .into(),
            ));
        }
        if let Some(icon) = &self.icon {
            if !root.join(icon).exists() {
                return Err(ClatchError::Invalid(format!(
                    "declared icon not found in the package: {icon}"
                )));
            }
        }
        if let Some(cli_bin) = self.connector.cli_bin() {
            if self.connector.cli_bin_in(root).is_none() {
                let also = if CLI_EXTENSIONS.is_empty() {
                    String::new()
                } else {
                    format!(" (nor with {})", CLI_EXTENSIONS.join(", "))
                };
                return Err(ClatchError::Invalid(format!(
                    "declared CLI binary not found in the package: {cli_bin}{also}"
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{resolve_exe, Manifest};

    fn parse(json: &str) -> Manifest {
        Manifest::parse(json).expect("parse")
    }

    /// A cli element is shipped by ONE package for every OS, so the CLI binary
    /// is RESOLVED like an executable, not matched like a filename. The
    /// extension list is per-OS; the search is not, so it is checked here on
    /// whatever machine runs the suite.
    #[test]
    fn the_cli_binary_resolves_through_the_hosts_executable_extensions() {
        let dir = std::env::temp_dir().join(format!("clatch-clibin-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let declared = dir.join("parts");
        assert_eq!(resolve_exe(declared.clone(), &["exe"]), None, "nothing yet");

        std::fs::write(dir.join("parts.exe"), "MZ").unwrap();
        assert_eq!(
            resolve_exe(declared.clone(), &["exe"]),
            Some(dir.join("parts.exe")),
            "a Windows package ships bin/parts.exe under `cli: parts`"
        );
        assert_eq!(
            resolve_exe(declared.clone(), &[]),
            None,
            "unix carries no extension, so it must not adopt an .exe it cannot run"
        );

        std::fs::write(&declared, "#!/bin/sh\n").unwrap();
        assert_eq!(
            resolve_exe(declared.clone(), &["exe"]),
            Some(declared),
            "the declared path always wins"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn locks_the_mandatory_fields() {
        let ok = parse(
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d",
                "version":"0.1.0", "protocol":2, "launch":{ "linux":"bin/a" }, "connector":{ "cli":"a" } }"#,
        );
        assert!(ok.validate().is_ok());

        // A present-but-empty mandatory string, or no launch command, is rejected.
        for bad in [
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"", "description":"d", "version":"1", "protocol":2, "launch":{"linux":"a"} }"#,
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"", "version":"1", "protocol":2, "launch":{"linux":"a"} }"#,
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d", "version":"", "protocol":2, "launch":{"linux":"a"} }"#,
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d", "version":"1", "protocol":2, "launch":{} }"#,
        ] {
            assert!(parse(bad).validate().is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn rejects_an_unsupported_protocol_major() {
        // The control-pipe major is gated at install (reference/protocol.md
        // § Versioning): a manifest targeting a newer major than this launcher
        // speaks is refused here, so a running instance never needs runtime
        // negotiation. 0 is invalid; anything past SUPPORTED_PROTOCOL is too new.
        for bad in [
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d", "version":"1", "protocol":0, "launch":{"linux":"a"}, "connector":{"cli":"a"} }"#,
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d", "version":"1", "protocol":9999, "launch":{"linux":"a"}, "connector":{"cli":"a"} }"#,
        ] {
            assert!(parse(bad).validate().is_err(), "should reject: {bad}");
        }
    }

    #[test]
    fn connector_cli_must_be_a_safe_segment() {
        // The cli becomes `bin/<cli>` (a symlink under ~/.clatch/bin) AND a
        // grant token `Bash(<cli>:*)`. A third-party clapp must not smuggle
        // traversal or grant-widening through it (2026-07-18).
        let base = |cli: &str| {
            format!(
                r#"{{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d",
                    "version":"0.1.0", "protocol":2, "launch":{{ "linux":"bin/a" }},
                    "connector":{{ "cli":{cli} }} }}"#
            )
        };
        // Clean shorthands pass.
        for good in [r#""arfchess""#, r#""jlc-app""#, r#""app.v2""#] {
            assert!(
                parse(&base(good)).validate().is_ok(),
                "should accept {good}"
            );
        }
        // Traversal, separators, a grant-breaking star or space are rejected.
        for bad in [
            r#""../../.local/bin/kubectl""#,
            r#""a/b""#,
            r#""a*""#,
            r#""a b""#,
            r#"".""#,
        ] {
            let err = parse(&base(bad)).validate().unwrap_err().to_string();
            assert!(err.contains("connector.cli"), "should reject {bad}: {err}");
        }
    }

    #[test]
    fn check_files_is_the_one_gate_install_and_validate_share() {
        use std::path::Path;
        // A manifest whose only launch command targets the OTHER OS has no path
        // to run here: check_files rejects it (install once accepted this while
        // validate rejected it - the exact divergence this gate closes).
        let other = if cfg!(target_os = "windows") {
            "linux"
        } else {
            "windows"
        };
        let no_host = parse(&format!(
            r#"{{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d", "version":"1", "protocol":2, "launch":{{ "{other}":"bin/a" }}, "connector":{{ "cli":"a" }} }}"#,
        ));
        assert!(
            no_host.validate().is_ok(),
            "a per-OS-only manifest is structurally valid"
        );
        assert!(
            no_host.check_files(Path::new("/nonexistent")).is_err(),
            "but it has no launch for this OS"
        );

        // A host-OS command whose file is missing is also rejected.
        let missing = parse(
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d", "version":"1", "protocol":2, "launch":{ "linux":"bin/a", "macos":"bin/a", "windows":"bin/a" } }"#,
        );
        assert!(missing.check_files(Path::new("/nonexistent")).is_err());
    }

    #[test]
    fn the_cli_is_the_clapp_constant_and_signals_never_fork_the_class() {
        // tools.md § Connectors (2026-07-18): every clapp ships its CLI (`-h`
        // is the floor), so a manifest without one is rejected; an EMPTY
        // signal set is a perfectly good clapp (optional facets are data,
        // never a different class).
        let no_cli = parse(
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d",
                "version":"1", "protocol":2, "launch":{"linux":"a"} }"#,
        );
        let err = no_cli.validate().unwrap_err().to_string();
        assert!(err.contains("connector.cli"), "{err}");

        let signalless = parse(
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d",
                "version":"1", "protocol":2, "launch":{"linux":"a"},
                "connector":{"cli":"a", "signals":[]} }"#,
        );
        assert!(
            signalless.validate().is_ok(),
            "a signalless clapp is a clapp"
        );

        // A declared but empty cli / signal name can never be used, so it is rejected.
        assert!(parse(r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d", "version":"1", "protocol":2, "launch":{"linux":"a"}, "connector":{"cli":""} }"#).validate().is_err());
        assert!(parse(r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d", "version":"1", "protocol":2, "launch":{"linux":"a"}, "connector":{"cli":"a", "signals":[{"id":"","type":"context"}]} }"#).validate().is_err());
    }

    #[test]
    fn unknown_fields_are_ignored_forward_compatible() {
        // A field a newer Clatch might add (a future banner, say) must not break an
        // older launcher's parse: the extension rule (data-structures.md).
        let m = parse(
            r#"{ "manifestVersion":1, "id":"com.x.a", "name":"A", "description":"d",
                "version":"1", "protocol":2, "launch":{"linux":"a"},
                "connector":{"cli":"a"},
                "banner":"assets/banner.png", "futureField":42 }"#,
        );
        assert!(m.validate().is_ok());
    }
}
