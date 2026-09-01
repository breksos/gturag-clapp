//! Spawning without leaking a console window.
//!
//! Clatch is a windowed app that spawns console programs: the GUI starts the
//! daemon, the daemon starts npm and the ACP adapters, an app relaunches itself
//! through the CLI. On Windows every one of those allocates a console unless
//! told not to, and the user watches black windows appear and vanish for the
//! life of the session (field report, 2026-07-19: "sürekli terminal açılıyor").
//!
//! The exception is deliberate and lives elsewhere: a vendor login TUI needs a
//! real console, so `backend_login_terminal` opens one on purpose.

use std::process::Command;

/// `CREATE_NO_WINDOW`. Not imported from a winapi crate: it is one stable
/// constant and this crate has no other reason to depend on one.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Spawn `command` without giving it a console of its own.
///
/// A no-op off Windows, where a child inherits the parent's terminal (or has
/// none) and nothing pops up either way.
pub fn hide_console(command: &mut Command) -> &mut Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command
}

/// Stop this process's std handles from being inherited by future children.
///
/// For the one child that outlives its parent: the detached daemon. On Windows
/// `CreateProcess` copies EVERY inheritable handle whenever stdio is redirected,
/// and a caller's stdio is often a pipe some grandparent reads to EOF (a test's
/// `Command::output`, an agent shell running `clatch ls`). The daemon then holds
/// that pipe's write end for its whole life, and the read outlives the CLI: the
/// command "hangs" long after its process exited. This was the first observed
/// Windows deadlock (reference/cross-platform.md B6): the CI test step sat 3h44m
/// in exactly this shape.
///
/// Clearing `HANDLE_FLAG_INHERIT` on our own std handles closes the leak at its
/// source and changes nothing else: the handles keep working here, and std
/// duplicates stdio per child rather than relying on this flag. Best-effort by
/// design (a handle that cannot be cleared just keeps the old behavior). A
/// no-op off Windows, where pipe fds are `O_CLOEXEC` and never survive the exec.
pub fn disinherit_stdio() {
    #[cfg(windows)]
    {
        const HANDLE_FLAG_INHERIT: u32 = 1;
        // STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE.
        const STD_HANDLES: [u32; 3] = [-10i32 as u32, -11i32 as u32, -12i32 as u32];
        // Not from a winapi crate, same rule as CREATE_NO_WINDOW above: two
        // stable kernel32 signatures are cheaper than a dependency.
        extern "system" {
            fn GetStdHandle(which: u32) -> *mut std::os::raw::c_void;
            fn SetHandleInformation(
                handle: *mut std::os::raw::c_void,
                mask: u32,
                flags: u32,
            ) -> i32;
        }
        for which in STD_HANDLES {
            // SAFETY: both calls take and return plain values; an invalid or
            // pseudo handle makes SetHandleInformation fail, which is the
            // documented best-effort outcome here.
            unsafe {
                let handle = GetStdHandle(which);
                if !handle.is_null() && handle as isize != -1 {
                    SetHandleInformation(handle, HANDLE_FLAG_INHERIT, 0);
                }
            }
        }
    }
}

/// Give `path` the execute bit (0o755). A no-op off unix, where execution
/// rides the file extension instead. The one copy: the CLI's launch stubs and
/// the daemon's backend shims each carried their own identical pair.
#[cfg(unix)]
pub fn make_executable(path: &std::path::Path) -> crate::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| crate::ClatchError::io(path, e))
}

/// See the unix arm; extensions carry executability here.
#[cfg(not(unix))]
pub fn make_executable(_path: &std::path::Path) -> crate::Result<()> {
    Ok(())
}
