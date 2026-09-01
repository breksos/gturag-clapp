//! The app library: install / list / uninstall, file-based, keyed by app id
//! (reference/systems.md). Steam's library, stripped to its essence, KISS, no
//! marketplace. The `source` argument is the only seam the future store touches.

use crate::manifest::Manifest;
use crate::record::RegistryRecord;
use clatch_core::{shim, AppId, ClatchError, Result};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone)]
pub struct Registry {
    home: PathBuf,
}

impl Registry {
    /// A registry rooted at an explicit `~/.clatch`-like home. Home resolution
    /// (env override, OS default) belongs to the caller at the edge, once.
    pub fn new(home: PathBuf) -> Self {
        Self { home }
    }

    /// The registry root (`~/.clatch`); siblings like `run/` derive from it.
    pub fn home(&self) -> &Path {
        &self.home
    }

    fn install_dir(&self, id: &AppId) -> PathBuf {
        self.home.join("apps").join(id.as_str())
    }
    fn registry_dir(&self) -> PathBuf {
        self.home.join("registry")
    }
    fn record_path(&self, id: &AppId) -> PathBuf {
        self.registry_dir().join(format!("{}.json", id.as_str()))
    }
    /// Clatch's own bin dir (`~/.clatch/bin`), on the daemon's PATH: one link per
    /// installed app's CLI, so an agent resolves the bare shorthand (reference/tools.md).
    fn bin_dir(&self) -> PathBuf {
        self.home.join("bin")
    }
    /// The app's durable-data home (`~/.clatch/appdata/<id>`,
    /// reference/protocol.md § Transport): injected at spawn as
    /// `CLATCH_DATA_DIR`, kept across uninstall/reinstall, erased by purge.
    pub fn data_dir(&self, id: &AppId) -> PathBuf {
        self.home.join("appdata").join(id.as_str())
    }

    /// Install from a local content folder containing `clatch.json`.
    pub fn install(&self, source: &Path) -> Result<RegistryRecord> {
        self.install_from(source, None)
    }

    /// [`Self::install`] with an explicit recorded `source` (launch.md
    /// § Distribution): `clapp:<path>` / `github:<owner>/<repo>@<tag>` when the
    /// content arrived through a depot, `None` = `local:<dir>`. The fetch and
    /// unpack happened before this call; the install path itself never differs.
    pub fn install_from(&self, source: &Path, origin: Option<String>) -> Result<RegistryRecord> {
        let manifest_path = source.join("clatch.json");
        let json =
            fs::read_to_string(&manifest_path).map_err(|e| ClatchError::io(&manifest_path, e))?;
        let manifest = Manifest::parse(&json)?;
        manifest.validate()?;
        let id = manifest.id.clone();
        // The cli shorthand the PREVIOUS install of this id linked, if any: a
        // reinstall that renames or drops its cli must not leave the old shim
        // on the agents' PATH, where it would dangle and block a later app
        // from legitimately claiming the name (the backend path learned the
        // same lesson as `purge_legacy`).
        let prior_cli = self.get(&id).ok().flatten().and_then(|r| r.cli);

        let dest = self.install_dir(&id);
        // Crash-atomic reinstall: stage the copy beside the live path, then
        // swap with one rename (the record, written last, stays the commit
        // mark). A kill mid-copy leaves only a stale staging dir, never a
        // half-copied app the registry still lists as installed. Mirrors the
        // temp + rename persistence rule.
        let staging = dest.with_file_name(format!("{id}.incoming"));
        if staging.exists() {
            clatch_core::fs::contended(|| fs::remove_dir_all(&staging))
                .map_err(|e| ClatchError::io(&staging, e))?;
        }
        copy_dir(source, &staging)?;
        // Reject a broken app at install, not at first run (a `dist` missing its
        // launch binary once installed "successfully" and left a dangling CLI
        // link, so the agent got `command not found`). The SAME gate `clatch
        // validate` uses, so the two can never disagree. Staging is torn down on
        // rejection - no partial install.
        if let Err(e) = manifest.check_files(&staging) {
            let _ = fs::remove_dir_all(&staging);
            return Err(e);
        }
        // Build the record and STAGE it beside its live path BEFORE the content
        // swap: the staged `<id>.json.incoming` is the roll-forward journal. If a
        // crash strikes between the swap and the commit, boot recovery
        // ([`Registry::recover_installs`]) finds this journal and finalizes the
        // install instead of leaving the OLD record describing the NEW content. It
        // is renamed into place LAST, atomically, which is the commit mark.
        let record = RegistryRecord::from_manifest(
            &manifest,
            dest.clone(),
            origin.unwrap_or_else(|| format!("local:{}", source.display())),
            chrono::Utc::now(),
        );
        let record_path = self.record_path(&id);
        let staged_record = with_suffix(&record_path, ".incoming");
        clatch_core::persist::save_json(&staged_record, &record)?;

        // Swap, keeping the old content aside as rollback material AND the staged
        // record as the roll-forward journal, UNTIL the commit rename lands. A
        // reinstall moves the live dir to `.outgoing` and the new content into
        // place (rename cannot replace a non-empty dir). The old dir is NOT
        // discarded here: a link/record failure below, or a crash, can still
        // restore it, so the registry never lists new content under an old record.
        let outgoing = dest.with_file_name(format!("{id}.outgoing"));
        if dest.exists() {
            if outgoing.exists() {
                clatch_core::fs::contended(|| fs::remove_dir_all(&outgoing))
                    .map_err(|e| ClatchError::io(&outgoing, e))?;
            }
            clatch_core::fs::contended(|| fs::rename(&dest, &outgoing))
                .map_err(|e| ClatchError::io(&dest, e))?;
        }
        if let Err(e) = clatch_core::fs::contended(|| fs::rename(&staging, &dest)) {
            // The content rename failed: the live dir is aside in `.outgoing` and
            // `dest` is gone. Put it back, discard the staged record, and surface
            // the error (all-or-nothing on this synchronous path).
            if outgoing.exists() {
                let _ = fs::rename(&outgoing, &dest);
            }
            let _ = fs::remove_file(&staged_record);
            return Err(ClatchError::io(&dest, e));
        }
        // Roll the old app back if anything before the commit fails: drop the new
        // content, restore `.outgoing`, discard the staged record. For a fresh
        // install `dest` did not exist, so this reduces to "no app installed".
        let restore = |dest: &Path, outgoing: &Path, staged: &Path| {
            let _ = clatch_core::fs::contended(|| fs::remove_dir_all(dest));
            if outgoing.exists() {
                let _ = clatch_core::fs::contended(|| fs::rename(outgoing, dest));
            }
            let _ = fs::remove_file(staged);
        };

        // Link the CLI onto the agent's PATH (reference/tools.md). The binary is
        // known to be there: `check_files` above is the gate, and it resolves the
        // host's executable extension, so a Windows package shipping `bin/x.exe`
        // links exactly like a unix one shipping `bin/x`. A name clash with
        // another installed app is a real conflict, so it rolls back rather than
        // commit a half-linked reinstall.
        if let (Some(cli), Some(target)) = (
            manifest.connector.cli.as_deref(),
            manifest.connector.cli_bin_in(&dest),
        ) {
            if let Err(e) = self.link_cli(&id, cli, &target) {
                restore(&dest, &outgoing, &staged_record);
                return Err(e);
            }
        }

        // COMMIT: one atomic rename of the staged record into place (same dir).
        // Before it, a crash rolls forward from the journal; after it, the record
        // matches the live content.
        if let Err(e) = fs::rename(&staged_record, &record_path) {
            // The commit never landed: unlink the shim we just placed (it points
            // into `dest`, which the rollback removes) and restore the old app.
            if let Some(cli) = manifest.connector.cli.as_deref() {
                self.unlink_cli(&id, cli);
            }
            restore(&dest, &outgoing, &staged_record);
            return Err(ClatchError::io(&record_path, e));
        }
        // COMMITTED. Only now is the old app safe to discard: reap the rollback
        // material, then drop a shim the reinstall superseded (its target still
        // points into this app's dir, so `unlink_cli`'s ownership check holds even
        // though the content behind it was swapped).
        if outgoing.exists() {
            let _ = fs::remove_dir_all(&outgoing); // best effort: the live app is already in place
        }
        if let Some(old) = prior_cli.filter(|c| record.cli.as_deref() != Some(c.as_str())) {
            self.unlink_cli(&id, &old);
        }
        Ok(record)
    }

    /// Recover from a reinstall interrupted by a process crash (reference/systems.md
    /// § Crash-atomic install). The reinstall stages the new record as
    /// `<id>.json.incoming` BEFORE swapping content, so at boot the journal decides
    /// the outcome; the content staging dir (`<id>.incoming`) tells whether the
    /// swap completed. Best-effort per id (a failure is logged, the rest recover)
    /// and idempotent, so running it every boot is safe.
    pub fn recover_installs(&self) {
        // Journalled ids: roll forward (commit) or back (discard) by whether the
        // content swap finished.
        if let Ok(entries) = fs::read_dir(self.registry_dir()) {
            for entry in entries.flatten() {
                let journal = entry.path();
                let Some(id_str) = journal
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| n.strip_suffix(".json.incoming"))
                else {
                    continue;
                };
                let id = AppId::new(id_str);
                let dest = self.install_dir(&id);
                let staging = dest.with_file_name(format!("{id_str}.incoming"));
                let outgoing = dest.with_file_name(format!("{id_str}.outgoing"));
                if staging.exists() {
                    // The swap never completed (new content still staged): roll
                    // BACK. Discard the journal + staging; if the live dir was
                    // already moved aside, put it back.
                    let _ = fs::remove_file(&journal);
                    let _ = fs::remove_dir_all(&staging);
                    if !dest.exists() && outgoing.exists() {
                        let _ = fs::rename(&outgoing, &dest);
                    }
                } else if let Err(e) = fs::rename(&journal, self.record_path(&id)) {
                    // The staging was consumed, so the new content is live: roll
                    // FORWARD by committing the staged record.
                    eprintln!("clatchd: recover {id_str}: finalize record: {e}");
                } else {
                    let _ = fs::remove_dir_all(&outgoing);
                }
            }
        }
        // Journal-less content markers (a crash before the journal, or after the
        // commit): reap. `.incoming` never went live; `.outgoing` is the old app
        // after a committed swap.
        if let Ok(entries) = fs::read_dir(self.home.join("apps")) {
            for entry in entries.flatten() {
                let path = entry.path();
                let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
                    continue;
                };
                if name.ends_with(".incoming") {
                    let _ = fs::remove_dir_all(&path);
                } else if let Some(id_str) = name.strip_suffix(".outgoing") {
                    let dest = self.install_dir(&AppId::new(id_str));
                    if dest.exists() {
                        let _ = fs::remove_dir_all(&path);
                    } else {
                        let _ = fs::rename(&path, &dest);
                    }
                }
            }
        }
    }

    /// Remove content + record; plain (`purge: false`) **keeps**
    /// `settings/<id>.json`, `stats/<id>.json`, and `appdata/<id>/` (Steam
    /// behavior: a reinstall restores them), while `purge: true` erases them
    /// too - the app's whole Clatch-known footprint (reference/daemon.md,
    /// app.rm). Stopping a running instance is lifecycle's job.
    pub fn uninstall(&self, id: &AppId, purge: bool) -> Result<()> {
        // The record is the commit mark (install writes it LAST): remove it
        // FIRST here, so the app is uninstalled the instant this returns and a
        // crash mid-cleanup leaves orphaned content (a harmless leak the next
        // install overwrites), never a phantom "Installed" app whose content is
        // half-gone. The CLI shorthand is read from the record before it dies.
        let cli = self.get(id)?.and_then(|r| r.cli);
        let rec = self.record_path(id);
        if rec.exists() {
            fs::remove_file(&rec).map_err(|e| ClatchError::io(&rec, e))?;
        }
        // Past the commit point: cleanup is best-effort (the app is already
        // gone; a leaked link or dir must not surface as an uninstall failure).
        // The CLI link goes first, while the content it points into still
        // exists (reference/tools.md).
        if let Some(cli) = cli {
            self.unlink_cli(id, &cli);
        }
        let dir = self.install_dir(id);
        if dir.exists() {
            if let Err(e) = clatch_core::fs::contended(|| fs::remove_dir_all(&dir)) {
                eprintln!(
                    "clatch: uninstalled {id} but left content at {}: {e}",
                    dir.display()
                );
            }
        }
        if purge {
            // Every path here derives from the validated id under Clatch's own
            // home (never user input as a path), and each removal stays
            // best-effort past the commit point, like the content above.
            for file in [
                self.home.join("settings").join(format!("{id}.json")),
                self.home.join("stats").join(format!("{id}.json")),
            ] {
                if file.exists() {
                    if let Err(e) = fs::remove_file(&file) {
                        eprintln!("clatch: purge {id}: left {}: {e}", file.display());
                    }
                }
            }
            let data = self.data_dir(id);
            if data.exists() {
                if let Err(e) = clatch_core::fs::contended(|| fs::remove_dir_all(&data)) {
                    eprintln!("clatch: purge {id}: left {}: {e}", data.display());
                }
            }
        }
        Ok(())
    }

    /// Link `~/.clatch/bin/<cli>` to this app's CLI binary (`target`, already known
    /// to exist). Fails only if the shorthand already belongs to another app.
    fn link_cli(&self, id: &AppId, cli: &str, target: &Path) -> Result<()> {
        let bin = self.bin_dir();
        fs::create_dir_all(&bin).map_err(|e| ClatchError::io(&bin, e))?;
        let link = shim::entry(&bin, cli);
        // `<home>/bin` has exactly two writers, so only two answers can refuse
        // this: the backend manager's entry is never an app's to take
        // (reference/install.md § Backend management), and another app's
        // shorthand is a genuine ambiguity. A stale link into *this* app is a
        // reinstall, which just replaces. The stamp gives the exact answer
        // (clatch_core::shim); the checks under it still read the generation
        // that predates the stamp - a symlink, or a body naming <home>/backends.
        let backends = self.home.join("backends");
        let reserved = || {
            ClatchError::Invalid(format!(
                "app {id}: cli `{cli}` is reserved by an installed backend"
            ))
        };
        if shim::writer_of(&link) == Some(shim::Writer::Backend) {
            return Err(reserved());
        }
        if let Some(current) = read_shim(&link) {
            if current.starts_with(&backends) {
                return Err(reserved());
            }
            if !current.starts_with(self.install_dir(id)) {
                return Err(ClatchError::Invalid(format!(
                    "app {id}: cli `{cli}` is already used by another installed app"
                )));
            }
        } else if link.exists()
            && fs::read_to_string(&link)
                .map(|c| c.contains(&backends.display().to_string()))
                .unwrap_or(false)
        {
            return Err(reserved());
        }
        // No remove-then-write: placement renames over whatever is there, so
        // the shorthand is never briefly absent and never half-written.
        write_shim(target, &link, &self.data_dir(id))
    }

    /// Remove `~/.clatch/bin/<cli>` only if it still points into this app (so a name
    /// clash reinstall never deletes the winner's link).
    fn unlink_cli(&self, id: &AppId, cli: &str) {
        let link = shim::entry(&self.bin_dir(), cli);
        if let Some(target) = read_shim(&link) {
            if target.starts_with(self.install_dir(id)) {
                let _ = fs::remove_file(&link);
            }
        }
    }

    pub fn get(&self, id: &AppId) -> Result<Option<RegistryRecord>> {
        let p = self.record_path(id);
        if !p.exists() {
            return Ok(None);
        }
        Ok(Some(read_record(&p)?))
    }

    /// [`get`](Self::get), but a missing app is the `NotFound` error every caller
    /// wants (the lifecycle, the daemon, the CLI all wrote the same
    /// `app <id> not installed`). One phrasing, one place.
    pub fn require(&self, id: &AppId) -> Result<RegistryRecord> {
        self.get(id)?
            .ok_or_else(|| ClatchError::NotFound(format!("app {id} not installed")))
    }

    pub fn list(&self) -> Result<Vec<RegistryRecord>> {
        let dir = self.registry_dir();
        let mut out = Vec::new();
        match fs::read_dir(&dir) {
            Ok(rd) => {
                for entry in rd {
                    let p = entry.map_err(|e| ClatchError::io(&dir, e))?.path();
                    if p.extension().and_then(|e| e.to_str()) == Some("json") {
                        out.push(read_record(&p)?);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(ClatchError::io(&dir, e)),
        }
        out.sort_by(|a, b| a.id.as_str().cmp(b.id.as_str()));
        Ok(out)
    }
}

/// Read + parse one record (the shared path for `get` and `list`).
fn read_record(path: &Path) -> Result<RegistryRecord> {
    let json = fs::read_to_string(path).map_err(|e| ClatchError::io(path, e))?;
    serde_json::from_str(&json)
        .map_err(|e| ClatchError::Invalid(format!("{}: {e}", path.display())))
}

/// Point `link` at `target` as an ENV-INJECTING exec wrapper: an `sh` script
/// on unix, a `.cmd` on Windows. The wrapper exports
/// `CLATCH_DATA_DIR=<appdata/<id>>` before exec, so an element's CLI running
/// in an AGENT's shell (where no per-app spawn env exists) still lands its
/// durable state where `--purge` can erase it - the purge promise holds for
/// every element type (reference/elements.md § cli).
///
/// Both formats compile and round-trip test on both platforms (`cfg!`, not
/// `#[cfg]`). A shim that is only ever compiled by the machine that needs it
/// is a shim nobody checks.
fn write_shim(target: &Path, link: &Path, data_dir: &Path) -> Result<()> {
    let body = if cfg!(unix) {
        sh_shim_contents(target, data_dir)
    } else {
        cmd_shim_contents(target, data_dir)
    };
    shim::place(link, &body)
}

/// What `link` points into, or `None` when it is missing or not ours. This is
/// the ownership question both callers ask: clash detection ("does this
/// shorthand already belong to another app?") and uninstall ("is this still
/// mine to delete?"). Reads every generation: the wrapper scripts (current)
/// AND the plain symlinks pre-wrapper installs left on unix.
fn read_shim(link: &Path) -> Option<PathBuf> {
    if let Ok(target) = fs::read_link(link) {
        return Some(target); // a pre-wrapper unix symlink
    }
    let body = fs::read_to_string(link).ok()?;
    if cfg!(unix) {
        parse_sh_shim(&body)
    } else {
        parse_cmd_shim(&body)
    }
}

/// The unix wrapper: export the data dir, then become the target (`exec`, so
/// no extra process lingers); `"$@"` passes every argument untouched.
fn sh_shim_contents(target: &Path, data_dir: &Path) -> String {
    format!(
        "#!/bin/sh\n{}export CLATCH_DATA_DIR=\"{}\"\nexec \"{}\" \"$@\"\n",
        shim::stamp_sh(shim::Writer::App),
        data_dir.display(),
        target.display()
    )
}

/// The `.cmd` body that makes `target` runnable under a bare name. `@` so cmd
/// does not echo, quotes so a spaced install path survives, `%*` so every
/// argument passes through untouched. No `cmd /C` anywhere, the agent's tool
/// call must never gain a shell (reference/cross-platform.md B4).
fn cmd_shim_contents(target: &Path, data_dir: &Path) -> String {
    format!(
        "{}@set \"CLATCH_DATA_DIR={}\"\r\n@\"{}\" %*\r\n",
        shim::stamp_cmd(shim::Writer::App),
        data_dir.display(),
        target.display()
    )
}

/// The target recorded in a wrapper body: the shim's answer to `read_link`.
/// Anything that does not parse is not a shim we wrote, i.e. no recorded
/// owner, and the two callers act on that differently ON PURPOSE: install
/// treats the shorthand as free and overwrites (self-healing a corrupted shim
/// in a directory only Clatch writes to), while uninstall leaves it alone,
/// since it must never delete a file it cannot prove is its own.
fn parse_cmd_shim(contents: &str) -> Option<PathBuf> {
    let line = contents.lines().find(|l| l.starts_with("@\""))?;
    let (target, _) = line.strip_prefix("@\"")?.split_once('"')?;
    Some(PathBuf::from(target))
}

/// The unix twin of [`parse_cmd_shim`]: the `exec "<target>" "$@"` line.
fn parse_sh_shim(contents: &str) -> Option<PathBuf> {
    let line = contents.lines().find(|l| l.starts_with("exec \""))?;
    let (target, _) = line.strip_prefix("exec \"")?.split_once('"')?;
    Some(PathBuf::from(target))
}

/// Append `suffix` to a path's full name (unlike `with_extension`, which
/// replaces the extension): `<id>.json` + `.incoming` -> `<id>.json.incoming`.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut s = path.as_os_str().to_os_string();
    s.push(suffix);
    PathBuf::from(s)
}

fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).map_err(|e| ClatchError::io(dst, e))?;
    for entry in fs::read_dir(src).map_err(|e| ClatchError::io(src, e))? {
        let entry = entry.map_err(|e| ClatchError::io(src, e))?;
        let to = dst.join(entry.file_name());
        if entry
            .file_type()
            .map_err(|e| ClatchError::io(&to, e))?
            .is_dir()
        {
            copy_dir(&entry.path(), &to)?;
        } else {
            fs::copy(entry.path(), &to).map_err(|e| ClatchError::io(&to, e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // The Windows shim is written and read back by code that a macOS-first team
    // never executes, so its format is pinned here, where every `cargo test`
    // runs it. `read_link` is the unix side of the same question and the OS
    // tests that one for us.

    #[test]
    fn a_shim_round_trips_to_its_target_on_both_formats() {
        let target = Path::new(r"C:\Users\x\.clatch\apps\com.arfium.arfchess\arfchess.exe");
        let data = Path::new(r"C:\Users\x\.clatch\appdata\com.arfium.arfchess");
        assert_eq!(
            parse_cmd_shim(&cmd_shim_contents(target, data)).as_deref(),
            Some(target)
        );
        let target = Path::new("/h/.clatch/apps/com.x.tool/bin/tool");
        let data = Path::new("/h/.clatch/appdata/com.x.tool");
        assert_eq!(
            parse_sh_shim(&sh_shim_contents(target, data)).as_deref(),
            Some(target)
        );
    }

    #[test]
    fn a_spaced_install_path_survives_the_shim() {
        // The whole reason the target is quoted: `C:\Program Files\...` must not
        // split into an executable and a stray argument.
        let target = Path::new(r"C:\Program Files\Clatch\apps\demo\demo.exe");
        let data = Path::new(r"C:\Users\x y\.clatch\appdata\demo");
        assert_eq!(
            parse_cmd_shim(&cmd_shim_contents(target, data)).as_deref(),
            Some(target)
        );
    }

    #[test]
    fn shims_inject_the_data_dir_and_pass_arguments_untouched() {
        // The wrapper is the purge promise for element CLIs run in agent
        // shells (reference/elements.md § cli): CLATCH_DATA_DIR is exported
        // before exec. `%*` / `"$@"`, and no `cmd /C`: the agent's tool call
        // must not gain a shell (reference/cross-platform.md B4).
        let body = cmd_shim_contents(Path::new(r"C:\x\demo.exe"), Path::new(r"C:\d"));
        assert!(body.contains(r#"@set "CLATCH_DATA_DIR=C:\d""#), "{body}");
        assert!(body.trim_end().ends_with(" %*"), "{body}");
        assert!(!body.contains("cmd"), "{body}");
        let body = sh_shim_contents(Path::new("/x/demo"), Path::new("/d"));
        assert!(body.contains("export CLATCH_DATA_DIR=\"/d\""), "{body}");
        assert!(body.contains("exec \"/x/demo\" \"$@\""), "{body}");
    }

    #[test]
    fn a_file_we_did_not_write_is_not_ours() {
        // `None` means no recorded owner. What must never happen is *guessing*
        // one: a wrong answer here lets uninstall delete another app's shim.
        assert_eq!(parse_cmd_shim(""), None);
        assert_eq!(parse_cmd_shim("echo hello\r\n"), None);
        assert_eq!(parse_cmd_shim("@echo off\r\n"), None);
        assert_eq!(parse_cmd_shim("@\"unterminated %*\r\n"), None);
        assert_eq!(parse_sh_shim("#!/bin/sh\necho hi\n"), None);
        assert_eq!(parse_sh_shim("exec \"unterminated\n"), None);
    }

    #[test]
    fn the_shorthand_is_resolvable_on_this_os() {
        let link = shim::entry(Path::new("/home/.clatch/bin"), "arfchess");
        if cfg!(windows) {
            // Without `.cmd` the bare name does not resolve through PATHEXT.
            assert_eq!(link.extension().and_then(|e| e.to_str()), Some("cmd"));
        } else {
            assert_eq!(link.file_name().and_then(|n| n.to_str()), Some("arfchess"));
        }
    }
}
