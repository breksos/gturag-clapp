//! `<home>/bin`: the shorthand namespace, and who placed what in it.
//!
//! Two writers share this one directory - the backend manager (ACP adapter and
//! vendor CLI launchers) and the app registry (an element's `cli` shorthand) -
//! and Clatch prepends it to `PATH`, so a name there can only mean one thing.
//! This module owns the three rules they must agree on: what an entry is
//! CALLED, how a Clatch-placed entry SAYS SO, and how one is written.
//!
//! The directory is Clatch's own: Clatch creates it and nothing else writes
//! there. So an entry carrying no stamp is not a stranger's file to be
//! preserved, it is a leftover of ours - an interrupted write, a build older
//! than the stamp, a file an antivirus touched - and refusing to touch it is
//! how a Windows user was left unable to install the default backend at all,
//! told by Clatch that Clatch had not placed it (field report, 2026-08-01,
//! `C:\Users\...\.clatch\bin\codex-acp.cmd`). Placement therefore ADOPTS an
//! unstamped entry; only the other writer's is refused, and removal stays
//! conservative (never delete what you cannot prove is yours).

use crate::error::{ClatchError, Result};
use std::fs;
use std::path::{Path, PathBuf};

/// The text every Clatch-placed shim carries. Deliberately not a path: a path
/// is a guess about layout that a rename, a case difference or a canonicalized
/// prefix can break, and it did.
const MARK: &str = "clatch-shim";

/// Only the head of an entry is read for the stamp: on unix a native backend
/// entry is a SYMLINK to a multi-megabyte binary, and no answer is worth
/// slurping that.
const HEAD: u64 = 512;

/// Which writer placed a `<home>/bin` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writer {
    /// The backend manager: an ACP adapter or a vendor CLI launcher.
    Backend,
    /// The app registry: an installed element's `cli` shorthand.
    App,
}

impl Writer {
    /// The word that names this writer inside the stamp.
    const fn tag(self) -> &'static str {
        match self {
            Writer::Backend => "backend",
            Writer::App => "app",
        }
    }
}

/// Where the shorthand `name` lives under `bin`. Windows needs the `.cmd`
/// extension for a bare name to resolve through `PATHEXT`; unix uses the name
/// itself (reference/cross-platform.md B2). One rule, one copy: the registry,
/// the backend manager and the daemon's element verbs must spell the same path
/// or they silently miss each other's files.
pub fn entry(bin: &Path, name: &str) -> PathBuf {
    if cfg!(windows) {
        bin.join(format!("{name}.cmd"))
    } else {
        bin.join(name)
    }
}

/// The stamp line for a `/bin/sh` wrapper (`#` is its comment).
pub fn stamp_sh(w: Writer) -> String {
    format!("# {MARK} {}\n", w.tag())
}

/// The stamp line for a `.cmd` wrapper (`@REM` is its comment, `@` so cmd does
/// not echo it). CRLF, because this is a batch file.
///
/// Spelled by SYNTAX, not by platform: the Windows body is written and unit
/// tested on machines that never run it, and a `cfg!`-picked comment character
/// would make those tests check the wrong file.
pub fn stamp_cmd(w: Writer) -> String {
    format!("@REM {MARK} {}\r\n", w.tag())
}

/// Who placed `link`, or `None` for an entry carrying no stamp - which in this
/// directory means a leftover, not a stranger (see the module note). A unix
/// symlink to a native binary also reads as unstamped; the backend manager's
/// own target check answers for those.
pub fn writer_of(link: &Path) -> Option<Writer> {
    let head = head_of(link)?;
    match head.split_once(MARK)?.1.split_whitespace().next()? {
        "backend" => Some(Writer::Backend),
        "app" => Some(Writer::App),
        _ => None,
    }
}

fn head_of(link: &Path) -> Option<String> {
    use std::io::Read as _;
    let mut buf = Vec::new();
    fs::File::open(link)
        .ok()?
        .take(HEAD)
        .read_to_end(&mut buf)
        .ok()?;
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// Write `body` as the `<home>/bin` entry `link`, atomically: a temp file
/// beside it, then one rename (which replaces an existing entry on both
/// platforms).
///
/// A shim is a few hundred bytes, but a plain write TRUNCATES first, so a
/// process death between truncate and write - an installer the user closes
/// mid-download, a machine that loses power - leaves a ZERO-BYTE shim that
/// carries no stamp. That is one of the ways a `<home>/bin` entry becomes
/// unrecognizable, and the cheapest to simply never create. The executable bit
/// lands before the entry is visible, for the same reason.
pub fn place(link: &Path, body: &str) -> Result<()> {
    let tmp = link.with_extension(format!("{}.tmp", std::process::id()));
    fs::write(&tmp, body).map_err(|e| ClatchError::io(&tmp, e))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Err(e) = fs::set_permissions(&tmp, fs::Permissions::from_mode(0o755)) {
            let _ = fs::remove_file(&tmp);
            return Err(ClatchError::io(&tmp, e));
        }
    }
    fs::rename(&tmp, link).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        ClatchError::io(link, e)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One directory PER TEST: they run concurrently, and a shared one had
    /// `placement_replaces...` scanning while another test's rename was still
    /// in flight.
    fn tmpdir(test: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("clatch-shim-{}-{test}", std::process::id()));
        let _ = fs::create_dir_all(&p);
        p
    }

    #[test]
    fn both_stamp_syntaxes_read_back_on_every_platform() {
        // The `.cmd` stamp is written by machines that never run cmd.exe.
        let dir = tmpdir("both47");
        for (name, body) in [
            (
                "sh",
                format!("#!/bin/sh\n{}exec x\n", stamp_sh(Writer::App)),
            ),
            (
                "cmd",
                format!("@ECHO off\r\n{}@\"x\" %*\r\n", stamp_cmd(Writer::App)),
            ),
        ] {
            let p = dir.join(format!("stamped-{name}"));
            place(&p, &body).unwrap();
            assert_eq!(writer_of(&p), Some(Writer::App), "{body}");
        }
        let backend = dir.join("stamped-backend");
        place(
            &backend,
            &format!("#!/bin/sh\n{}", stamp_sh(Writer::Backend)),
        )
        .unwrap();
        assert_eq!(writer_of(&backend), Some(Writer::Backend));
    }

    #[test]
    fn the_leftovers_a_field_report_produced_read_as_unstamped() {
        // Each of these was a permanent refusal before: nothing in this
        // directory may be able to brick an install.
        let dir = tmpdir("the55");
        let empty = dir.join("empty");
        fs::write(&empty, "").unwrap();
        assert_eq!(writer_of(&empty), None, "a truncated write");
        let old = dir.join("old-format");
        fs::write(&old, "@ECHO off\r\n@\"C:\\some\\old\\path.exe\" %*\r\n").unwrap();
        assert_eq!(writer_of(&old), None, "a build older than the stamp");
        let binary = dir.join("binary");
        fs::write(&binary, [0x7f, b'E', b'L', b'F', 0, 1, 2, 3]).unwrap();
        assert_eq!(writer_of(&binary), None, "not text at all");
        assert_eq!(writer_of(&dir.join("absent")), None, "no entry");
    }

    #[test]
    fn placement_replaces_and_never_leaves_a_temp_file() {
        let dir = tmpdir("placement47");
        let link = dir.join("replaced.cmd");
        fs::write(&link, "old").unwrap();
        place(&link, "new").unwrap();
        assert_eq!(fs::read_to_string(&link).unwrap(), "new");
        let strays: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp"))
            .collect();
        assert!(strays.is_empty(), "temp files left behind: {strays:?}");
    }

    #[test]
    fn the_name_carries_the_windows_extension_and_nothing_else() {
        let got = entry(Path::new("/h/bin"), "codex-acp");
        let want = if cfg!(windows) {
            "codex-acp.cmd"
        } else {
            "codex-acp"
        };
        assert_eq!(got.file_name().unwrap().to_str().unwrap(), want);
    }
}
