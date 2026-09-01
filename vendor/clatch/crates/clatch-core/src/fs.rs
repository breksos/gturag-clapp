//! Filesystem mutations that another process can be holding open.
//!
//! On unix a directory can be renamed or removed while files inside it are
//! open. Windows cannot: an antivirus scanning a freshly unpacked app, the
//! search indexer, or Explorer showing the folder all hold handles, and the
//! mutation fails with a sharing violation - for the fraction of a second the
//! scanner needs, not permanently. An install that gives up on that is an
//! install that fails at random on one platform only.
//!
//! One helper, used by the paths that move or delete whole directories
//! (reference/cross-platform.md § Dir mutations vs open handles).

use std::io;
use std::time::Duration;

/// How many times a contended mutation is re-tried, and how long it waits
/// between attempts. Short and bounded: a scanner releases in milliseconds, and
/// a handle that outlives this is a real conflict the caller must hear about.
const BACKOFF: &[Duration] = &[
    Duration::from_millis(30),
    Duration::from_millis(90),
    Duration::from_millis(250),
];

/// Run `op`, re-trying while it fails the way a held handle fails.
///
/// Only contention is re-tried; a missing path or a real permission error
/// returns on the first attempt, because retrying those is just latency added
/// to an answer that will not change. The final failure is the caller's to
/// report, unchanged.
pub fn contended<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    let mut attempt = 0;
    loop {
        match op() {
            Ok(v) => return Ok(v),
            Err(e) if attempt < BACKOFF.len() && is_contention(&e) => {
                std::thread::sleep(BACKOFF[attempt]);
                attempt += 1;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Does this error look like someone else holding the path? `PermissionDenied`
/// is what Windows reports for a sharing violation (`ERROR_ACCESS_DENIED`, and
/// `ERROR_SHARING_VIOLATION` = 32, which std does not map to a named kind);
/// `DirectoryNotEmpty` is the shape a directory delete takes when a scanner
/// re-creates or holds an entry while it is being emptied.
fn is_contention(e: &io::Error) -> bool {
    matches!(
        e.kind(),
        io::ErrorKind::PermissionDenied | io::ErrorKind::DirectoryNotEmpty
    ) || e.raw_os_error() == Some(32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn contention_is_ridden_out_and_everything_else_returns_at_once() {
        let tries = Cell::new(0);
        let out = contended(|| {
            tries.set(tries.get() + 1);
            if tries.get() < 3 {
                Err(io::Error::from(io::ErrorKind::PermissionDenied))
            } else {
                Ok(7)
            }
        });
        assert_eq!((out.unwrap(), tries.get()), (7, 3), "the scanner let go");

        let tries = Cell::new(0);
        let out: io::Result<()> = contended(|| {
            tries.set(tries.get() + 1);
            Err(io::Error::from(io::ErrorKind::NotFound))
        });
        assert!(out.is_err());
        assert_eq!(tries.get(), 1, "a missing path is not contention");

        let tries = Cell::new(0);
        let out: io::Result<()> = contended(|| {
            tries.set(tries.get() + 1);
            Err(io::Error::from(io::ErrorKind::PermissionDenied))
        });
        assert!(out.is_err());
        assert_eq!(
            tries.get(),
            BACKOFF.len() + 1,
            "a handle that never lets go is a real failure"
        );
    }
}
