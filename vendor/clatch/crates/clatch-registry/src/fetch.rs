//! Distribution fetch: HTTPS download, sha256 verify, archive extract.
//!
//! The one download/verify/extract helper the install paths share (backends'
//! portable Node + npm registry today; `.clapp` release assets next,
//! reference/launch.md § Distribution). Deliberately blocking (the callers run
//! it off the async core via `spawn_blocking`) and deliberately light: `ureq`
//! over a full client stack; HTTPS only; nothing here ever executes what it
//! downloaded.

use clatch_core::{ClatchError, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::Path;

/// Refuse anything larger than this (a corrupt length header must not fill the
/// disk); the biggest legitimate artifact today is a Node tarball (~55 MB).
const MAX_BYTES: u64 = 512 * 1024 * 1024;

fn invalid(what: &str, detail: impl std::fmt::Display) -> ClatchError {
    ClatchError::Invalid(format!("{what}: {detail}"))
}

/// Every request identifies itself (GitHub's API requires a User-Agent).
const UA: &str = concat!("clatch/", env!("CARGO_PKG_VERSION"));

/// The one HTTP agent every fetch rides. ureq's DEFAULTS HAVE NO READ TIMEOUT
/// (a stalled peer would block a `read_to_end` forever, wedging the install
/// lock, the observed fail-safe violation), so every timeout is explicit:
/// per-read, not whole-request, so a slow-but-flowing big download survives
/// while a stall dies in a minute. `https_only` also kills a redirect
/// downgrading to http, which the entry checks alone cannot see.
fn agent() -> &'static ureq::Agent {
    static AGENT: std::sync::OnceLock<ureq::Agent> = std::sync::OnceLock::new();
    AGENT.get_or_init(|| {
        ureq::AgentBuilder::new()
            .timeout_connect(std::time::Duration::from_secs(15))
            .timeout_read(std::time::Duration::from_secs(60))
            .timeout_write(std::time::Duration::from_secs(60))
            .https_only(true)
            .user_agent(UA)
            // NO connection pooling: ureq 2.12 clears the socket timeouts when a
            // stream returns to the pool (`Stream::reset`) and never re-arms them
            // on reuse, so a pooled request's header read can block forever, the
            // exact hang the timeouts above exist to kill. Our few sequential
            // requests happily pay a fresh TLS handshake each.
            .max_idle_connections(0)
            .max_idle_connections_per_host(0)
            .build()
    })
}

/// Download `url` (HTTPS only) into memory, bounded by `MAX_BYTES`.
pub fn fetch(url: &str) -> Result<Vec<u8>> {
    fetch_with_progress(url, &|_, _| {})
}

/// [`fetch`], reporting download progress as it streams: `on_bytes(done, total)`
/// fires as the body arrives, with `total` the `Content-Length` (or `0` when the
/// server sends none). Throttled to one call per `PROGRESS_STEP` bytes so a big
/// download does not flood the progress channel; the final byte always fires.
pub fn fetch_with_progress(url: &str, on_bytes: &dyn Fn(u64, u64)) -> Result<Vec<u8>> {
    if !url.starts_with("https://") {
        return Err(invalid("fetch", format!("not https: {url}")));
    }
    let resp = agent()
        .get(url)
        .call()
        .map_err(|e| invalid("fetch", format!("{url}: {e}")))?;
    let total: u64 = resp
        .header("Content-Length")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let mut reader = resp.into_reader().take(MAX_BYTES);
    let mut body = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    let mut emitted = 0u64;
    loop {
        let n = reader
            .read(&mut buf)
            .map_err(|e| invalid("fetch", format!("{url}: {e}")))?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&buf[..n]);
        let done = body.len() as u64;
        if done - emitted >= PROGRESS_STEP {
            emitted = done;
            on_bytes(done, total);
        }
    }
    if body.len() as u64 >= MAX_BYTES {
        return Err(invalid(
            "fetch",
            format!("{url}: larger than {MAX_BYTES} bytes"),
        ));
    }
    on_bytes(body.len() as u64, total);
    Ok(body)
}

/// Emit a download tick at most this often (bytes). Small enough that a bar
/// moves smoothly, large enough that a 55 MB Node tarball is ~100 updates.
const PROGRESS_STEP: u64 = 512 * 1024;

/// GET a JSON API endpoint (HTTPS only), with an optional bearer token. A 403
/// from GitHub names the unauthenticated rate limit (60/h) and the
/// `CLATCH_GITHUB_TOKEN` remedy, so the failure is actionable.
pub fn fetch_json(url: &str, bearer: Option<&str>) -> Result<serde_json::Value> {
    if !url.starts_with("https://") {
        return Err(invalid("fetch", format!("not https: {url}")));
    }
    let mut req = agent()
        .get(url)
        .set("Accept", "application/vnd.github+json");
    if let Some(token) = bearer {
        req = req.set("Authorization", &format!("Bearer {token}"));
    }
    let resp = req.call().map_err(|e| match e {
        ureq::Error::Status(403, _) if url.contains("api.github.com") => invalid(
            "fetch",
            format!(
                "{url}: 403 (GitHub's unauthenticated API limit is 60 requests/hour; \
                 set CLATCH_GITHUB_TOKEN to raise it)"
            ),
        ),
        e => invalid("fetch", format!("{url}: {e}")),
    })?;
    let mut body = String::new();
    resp.into_reader()
        .take(MAX_BYTES)
        .read_to_string(&mut body)
        .map_err(|e| invalid("fetch", format!("{url}: {e}")))?;
    serde_json::from_str(&body).map_err(|e| invalid("fetch", format!("{url}: bad json: {e}")))
}

/// Lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Verify `bytes` against an expected lowercase-hex sha256, with a loud
/// mismatch naming what was being verified.
pub fn verify_sha256(what: &str, bytes: &[u8], expected: &str) -> Result<()> {
    let actual = sha256_hex(bytes);
    if actual == expected.trim().to_lowercase() {
        Ok(())
    } else {
        Err(invalid(
            "sha256 mismatch",
            format!("{what}: expected {expected}, got {actual}"),
        ))
    }
}

/// Extract a `.tar.gz` into `dest` (created if missing). Preserves the archive's
/// unix modes (the +x bits a runtime's `bin/` needs); rejects entries escaping
/// `dest` (the `tar` crate enforces path containment on unpack). NO output-size
/// ceiling of its own: callers feed it **verified bytes only** (the pinned-sha
/// Node runtime today); an unverified archive goes through `clapp::unpack`,
/// which does enforce one.
pub fn untar_gz(bytes: &[u8], dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| ClatchError::io(dest, e))?;
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes));
    archive
        .unpack(dest)
        .map_err(|e| invalid("untar", format!("{}: {e}", dest.display())))
}

/// Extract a `.zip` into `dest` (created if missing). The sibling of
/// [`untar_gz`] for the one platform Node ships as a zip: Windows publishes
/// `node-v<version>-win-<arch>.zip` and no tarball at all
/// (reference/cross-platform.md B5). Same contract, same caller obligation:
/// **verified bytes only**, so there is no output-size ceiling here either.
///
/// Unlike `tar`, the `zip` crate does not enforce containment for us, so entry
/// names are checked here: `enclosed_name()` returns `None` for anything with a
/// traversal component or an absolute path, and we refuse rather than sanitize.
/// Silently rewriting a hostile name is how an escape becomes a surprise.
pub fn unzip(bytes: &[u8], dest: &Path) -> Result<()> {
    fs::create_dir_all(dest).map_err(|e| ClatchError::io(dest, e))?;
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
        .map_err(|e| invalid("unzip", format!("{}: {e}", dest.display())))?;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| invalid("unzip", format!("entry {i}: {e}")))?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(invalid(
                "unzip",
                format!("entry {i} escapes the destination: {}", entry.name()),
            ));
        };
        let out = dest.join(rel);
        if entry.is_dir() {
            fs::create_dir_all(&out).map_err(|e| ClatchError::io(&out, e))?;
            continue;
        }
        if let Some(parent) = out.parent() {
            fs::create_dir_all(parent).map_err(|e| ClatchError::io(parent, e))?;
        }
        let mut file = fs::File::create(&out).map_err(|e| ClatchError::io(&out, e))?;
        std::io::copy(&mut entry, &mut file).map_err(|e| ClatchError::io(&out, e))?;
        // Node's zip carries unix modes for its shell wrappers; honour them where
        // they exist so a cross-platform unpack keeps the +x bits.
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&out, fs::Permissions::from_mode(mode))
                .map_err(|e| ClatchError::io(&out, e))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sha256_is_the_reference_vector() {
        // The canonical empty-input and "abc" NIST vectors.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        verify_sha256(
            "abc",
            b"abc",
            "BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD",
        )
        .expect("case-insensitive match");
        assert!(verify_sha256("abc", b"abc", "deadbeef").is_err());
    }

    #[test]
    fn fetch_refuses_plain_http() {
        let err = fetch("http://example.com/x").unwrap_err().to_string();
        assert!(err.contains("not https"), "{err}");
    }

    #[test]
    fn untar_gz_round_trips_a_tree_with_modes() {
        // Build a tar.gz in memory: bin/tool (0o755) + doc.txt (0o644).
        let mut tar_bytes = Vec::new();
        {
            let enc = flate2::write::GzEncoder::new(&mut tar_bytes, flate2::Compression::fast());
            let mut b = tar::Builder::new(enc);
            let mut h = tar::Header::new_gnu();
            h.set_size(3);
            h.set_mode(0o755);
            h.set_cksum();
            b.append_data(&mut h, "pkg/bin/tool", &b"#!x"[..]).unwrap();
            let mut h = tar::Header::new_gnu();
            h.set_size(2);
            h.set_mode(0o644);
            h.set_cksum();
            b.append_data(&mut h, "pkg/doc.txt", &b"ok"[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let dir = clatch_testkit::tmp();
        untar_gz(&tar_bytes, &dir).expect("unpack");
        let tool = dir.join("pkg/bin/tool");
        assert_eq!(fs::read(&tool).unwrap(), b"#!x");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&tool).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "exec bits survive: {mode:o}");
        }
    }
}
