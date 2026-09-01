//! `.clapp` per-platform depots (reference/launch.md § Distribution).
//!
//! A `.clapp` is a zip rooted at `clatch.json` carrying ONE platform's payload,
//! named `<id>-<os>-<arch>.clapp`. This module packs a content folder into the
//! host depot, unpacks one for install (zip-slip guarded), and resolves a GitHub
//! release's host asset by the documented naming convention. `stage` is the one
//! entry the daemon's `app.install` calls: spec in, unpacked staging dir +
//! recorded `source` out. Nothing here ever executes content.

use crate::fetch;
use crate::manifest::Manifest;
use clatch_core::{ClatchError, Progress, Result};
use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

fn invalid(msg: impl Into<String>) -> ClatchError {
    ClatchError::Invalid(msg.into())
}

/// The depot naming's host pair (`launch.md`: os in macos/linux/windows,
/// arch in arm64/x64).
pub fn host_platform() -> Result<(&'static str, &'static str)> {
    let os = match std::env::consts::OS {
        "macos" => "macos",
        "linux" => "linux",
        "windows" => "windows",
        other => return Err(invalid(format!("no depot mapping for OS {other}"))),
    };
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86_64" => "x64",
        other => return Err(invalid(format!("no depot mapping for arch {other}"))),
    };
    Ok((os, arch))
}

/// Pack `dir` (a validated content folder) into `<out>/<id>-<os>-<arch>.clapp`.
/// The same file gate as install/validate runs first, so a depot that would be
/// rejected on install cannot be built. `.git` and `.DS_Store` never ship.
pub fn pack(dir: &Path, out: &Path) -> Result<PathBuf> {
    let manifest_path = dir.join("clatch.json");
    let json =
        fs::read_to_string(&manifest_path).map_err(|e| ClatchError::io(&manifest_path, e))?;
    let manifest = Manifest::parse(&json)?;
    manifest.validate()?;
    manifest.check_files(dir)?;

    let (os, arch) = host_platform()?;
    let path = out.join(format!("{}-{os}-{arch}.clapp", manifest.id));
    let file = fs::File::create(&path).map_err(|e| ClatchError::io(&path, e))?;
    let mut zip = zip::ZipWriter::new(file);
    add_tree(&mut zip, dir, Path::new("")).inspect_err(|_| {
        let _ = fs::remove_file(&path);
    })?;
    zip.finish().map_err(|e| invalid(format!("pack: {e}")))?;
    Ok(path)
}

fn add_tree(zip: &mut zip::ZipWriter<fs::File>, root: &Path, rel: &Path) -> Result<()> {
    let dir = root.join(rel);
    let mut entries: Vec<_> = fs::read_dir(&dir)
        .map_err(|e| ClatchError::io(&dir, e))?
        .filter_map(|e| e.ok())
        .collect();
    entries.sort_by_key(|e| e.file_name()); // deterministic archives
    for entry in entries {
        let name = entry.file_name();
        if name == ".git" || name == ".DS_Store" {
            continue;
        }
        let rel = rel.join(&name);
        let path = root.join(&rel);
        let meta = fs::symlink_metadata(&path).map_err(|e| ClatchError::io(&path, e))?;
        if meta.is_dir() {
            add_tree(zip, root, &rel)?;
        } else if meta.is_file() {
            #[cfg(unix)]
            let mode = {
                use std::os::unix::fs::PermissionsExt;
                meta.permissions().mode()
            };
            #[cfg(not(unix))]
            let mode = 0o644;
            let opts = zip::write::SimpleFileOptions::default().unix_permissions(mode);
            zip.start_file(rel.to_string_lossy().replace('\\', "/"), opts)
                .map_err(|e| invalid(format!("pack {}: {e}", rel.display())))?;
            let bytes = fs::read(&path).map_err(|e| ClatchError::io(&path, e))?;
            zip.write_all(&bytes)
                .map_err(|e| ClatchError::io(&path, e))?;
        }
        // Symlinks are skipped: a depot ships real files only.
    }
    Ok(())
}

/// The uncompressed ceiling per `.clapp`. A depot is one app's binaries; a
/// couple-hundred-KB zip inflating past this is a decompression bomb, not an
/// app (the zip headers' own size claims are attacker data, so the LIVE copy
/// is what gets bounded, never `ZipFile::size()`).
const MAX_UNPACKED: u64 = 4 * 1024 * 1024 * 1024;

/// Unpack a `.clapp` file into `dest` (created). See [`unpack_from`].
pub fn unpack(clapp: &Path, dest: &Path) -> Result<()> {
    let file = fs::File::open(clapp).map_err(|e| ClatchError::io(clapp, e))?;
    unpack_from(file, dest)
}

/// Unpack a `.clapp` archive into `dest`: every entry must stay inside `dest`
/// (an escaping name is a loud error, never skipped silently), the total
/// uncompressed payload is capped (`MAX_UNPACKED`), unix modes are preserved
/// (the launch binary's +x), and the root `clatch.json` is required.
pub fn unpack_from<R: Read + Seek>(reader: R, dest: &Path) -> Result<()> {
    unpack_with_limit(reader, dest, MAX_UNPACKED)
}

fn unpack_with_limit<R: Read + Seek>(reader: R, dest: &Path, limit: u64) -> Result<()> {
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| invalid(format!("not a .clapp (zip): {e}")))?;
    fs::create_dir_all(dest).map_err(|e| ClatchError::io(dest, e))?;
    let mut remaining = limit;
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| invalid(format!("unpack: {e}")))?;
        let Some(rel) = entry.enclosed_name() else {
            return Err(invalid(format!(
                "unpack: entry '{}' escapes the target directory",
                entry.name()
            )));
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
        let written = std::io::copy(&mut (&mut entry).take(remaining + 1), &mut file)
            .map_err(|e| ClatchError::io(&out, e))?;
        if written > remaining {
            return Err(invalid(format!(
                "unpack: uncompressed payload exceeds the {limit}-byte ceiling \
                 (a decompression bomb, or not an app depot)"
            )));
        }
        remaining -= written;
        #[cfg(unix)]
        if let Some(mode) = entry.unix_mode() {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&out, fs::Permissions::from_mode(mode))
                .map_err(|e| ClatchError::io(&out, e))?;
        }
    }
    if !dest.join("clatch.json").exists() {
        return Err(invalid("not a .clapp: no clatch.json at the archive root"));
    }
    Ok(())
}

/// A parsed GitHub install spec: `owner/repo[@tag]`, a repo URL, or a release
/// URL. `None` = not GitHub-shaped (the caller falls through to other forms).
#[derive(Debug, PartialEq, Eq)]
pub struct GithubSpec {
    pub owner: String,
    pub repo: String,
    pub tag: Option<String>,
}

pub fn parse_github(spec: &str) -> Option<GithubSpec> {
    let seg = |s: &str| {
        !s.is_empty()
            && !s.chars().all(|c| c == '.') // ".."/"." are path steps, never repo names
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    // URL forms: https://github.com/owner/repo[.git][/releases/tag/<tag>|/releases/latest]
    if let Some(rest) = spec
        .strip_prefix("https://github.com/")
        .or_else(|| spec.strip_prefix("github.com/"))
    {
        let parts: Vec<&str> = rest.trim_end_matches('/').split('/').collect();
        let (owner, repo) = match parts.as_slice() {
            [o, r, ..] => ((*o).to_string(), r.trim_end_matches(".git").to_string()),
            _ => return None,
        };
        if !seg(&owner) || !seg(&repo) {
            return None;
        }
        let tag = match parts.as_slice() {
            [_, _, "releases", "tag", t] if seg(t) => Some((*t).to_string()),
            [_, _] | [_, _, "releases", "latest"] => None,
            _ => return None, // a deeper GitHub URL is not an install spec
        };
        return Some(GithubSpec { owner, repo, tag });
    }
    // Short form: owner/repo[@tag]. A path that exists on disk is never treated
    // as a repo (folders stay folders).
    if spec.contains("://") || Path::new(spec).exists() {
        return None;
    }
    let (base, tag) = match spec.split_once('@') {
        Some((b, t)) if seg(t) => (b, Some(t.to_string())),
        Some(_) => return None,
        None => (spec, None),
    };
    match base.split('/').collect::<Vec<_>>().as_slice() {
        [o, r] if seg(o) && seg(r) => Some(GithubSpec {
            owner: (*o).to_string(),
            repo: (*r).to_string(),
            tag,
        }),
        _ => None,
    }
}

/// Pick the host depot from a release's asset names (`launch.md`, widest match
/// last): `-<os>-<arch>.clapp`, then `-<os>-any.clapp`, then a single
/// cross-platform `.clapp` (no platform marker). No match is a loud error
/// listing what the release ships.
pub fn pick_asset<'a>(names: &'a [String], os: &str, arch: &str) -> Result<&'a str> {
    let clapps: Vec<&String> = names.iter().filter(|n| n.ends_with(".clapp")).collect();
    for suffix in [format!("-{os}-{arch}.clapp"), format!("-{os}-any.clapp")] {
        if let Some(n) = clapps.iter().find(|n| n.ends_with(&suffix)) {
            return Ok(n);
        }
    }
    let markerless: Vec<&&String> = clapps
        .iter()
        .filter(|n| {
            !["macos", "linux", "windows"]
                .iter()
                .any(|o| n.contains(&format!("-{o}-")))
        })
        .collect();
    match markerless.as_slice() {
        [one] => Ok(one),
        _ => Err(invalid(format!(
            "no .clapp for {os}-{arch} in this release (assets: {})",
            if clapps.is_empty() {
                names.join(", ")
            } else {
                clapps
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        ))),
    }
}

/// One resolved install source, ready for the actor to install identically
/// however it arrived: the content `dir`, the `source` string to record, and
/// whether `dir` is a throwaway staging tree the caller must delete (a fetched
/// or unpacked depot) or the user's own folder (kept).
#[derive(Debug)]
pub struct Staged {
    pub dir: PathBuf,
    pub source: String,
    pub temp: bool,
}

/// Resolve any install `spec` into a [`Staged`] the actor installs the same way:
/// a local content folder (handed back untouched), a `.clapp` file, or a GitHub
/// release (both unpacked into a `<home>/tmp` staging tree). The one resolver
/// every install source funnels through; blocking (network + IO), progress
/// (lines + download fractions) flows to `progress`.
pub fn stage(home: &Path, spec: &str, progress: &dyn Fn(Progress)) -> Result<Staged> {
    // A local content folder is already staged: hand it back untouched, so a
    // large app is copied once (into place, by install_from), not twice.
    if Path::new(spec).is_dir() {
        progress(format!("installing from {spec}").into());
        return Ok(Staged {
            dir: PathBuf::from(spec),
            source: format!("local:{spec}"),
            temp: false,
        });
    }
    let staging = fresh_staging(home)?;
    let done = |r: Result<String>| -> Result<Staged> {
        match r {
            Ok(source) => Ok(Staged {
                dir: staging.clone(),
                source,
                temp: true,
            }),
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                Err(e)
            }
        }
    };
    if spec.ends_with(".clapp") {
        let path = Path::new(spec);
        if !path.is_file() {
            return done(Err(invalid(format!("no such .clapp file: {spec}"))));
        }
        progress(format!("unpacking {}", path.display()).into());
        return done(unpack(path, &staging).map(|()| format!("clapp:{}", path.display())));
    }
    if let Some(gh) = parse_github(spec) {
        return done(stage_github(&gh, &staging, progress));
    }
    done(Err(invalid(format!(
        "'{spec}' is not an install source: expected a folder, a .clapp file, \
         or a GitHub repo (owner/repo[@tag] or a github.com URL)"
    ))))
}

fn stage_github(gh: &GithubSpec, staging: &Path, progress: &dyn Fn(Progress)) -> Result<String> {
    let token = std::env::var("CLATCH_GITHUB_TOKEN").ok();
    let api = match &gh.tag {
        Some(t) => format!(
            "https://api.github.com/repos/{}/{}/releases/tags/{t}",
            gh.owner, gh.repo
        ),
        None => format!(
            "https://api.github.com/repos/{}/{}/releases/latest",
            gh.owner, gh.repo
        ),
    };
    progress(format!("resolving {}/{} release", gh.owner, gh.repo).into());
    let release = fetch::fetch_json(&api, token.as_deref())?;
    let tag = release["tag_name"]
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let assets: Vec<(String, String)> = release["assets"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    Some((
                        x["name"].as_str()?.to_string(),
                        x["browser_download_url"].as_str()?.to_string(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();
    let names: Vec<String> = assets.iter().map(|(n, _)| n.clone()).collect();
    let (os, arch) = host_platform()?;
    let asset = pick_asset(&names, os, arch)?.to_string();
    let url = &assets.iter().find(|(n, _)| *n == asset).expect("picked").1;

    progress(format!("downloading {asset} ({tag})").into());
    let bytes =
        fetch::fetch_with_progress(url, &|done, total| progress(Progress::frac(done, total)))?;

    // A sibling `<asset>.sha256` comes from the SAME release, so matching it
    // proves the download arrived intact (a corruption check), NOT that anyone
    // trustworthy made it (authenticity would need a signature, which is
    // deliberately out of v1 scope). The wording must never imply more.
    if let Some((_, sha_url)) = assets.iter().find(|(n, _)| *n == format!("{asset}.sha256")) {
        progress("checking sha256 (corruption check, not a signature)".into());
        let sums = String::from_utf8_lossy(&fetch::fetch(sha_url)?).into_owned();
        let expected = sums.split_whitespace().next().unwrap_or("").to_string();
        fetch::verify_sha256(&asset, &bytes, &expected)?;
    } else {
        progress("no .sha256 asset in the release; relying on https".into());
    }

    progress("unpacking".into());
    unpack_from(std::io::Cursor::new(bytes), staging)?;
    Ok(format!("github:{}/{}@{tag}", gh.owner, gh.repo))
}

/// A fresh unique staging dir under `<home>/tmp` (torn down by the caller).
fn fresh_staging(home: &Path) -> Result<PathBuf> {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let dir = home
        .join("tmp")
        .join(format!("install-{}-{nanos}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|e| ClatchError::io(&dir, e))?;
    Ok(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_then_unpack_round_trips_the_tree_with_modes() {
        let src = clatch_testkit::tmp();
        clatch_testkit::write_app(&src, "com.x.depot");
        let out = clatch_testkit::tmp();
        let clapp = pack(&src, &out).expect("pack");
        let (os, arch) = host_platform().unwrap();
        assert_eq!(
            clapp.file_name().unwrap().to_str().unwrap(),
            format!("com.x.depot-{os}-{arch}.clapp")
        );

        let dest = clatch_testkit::tmp().join("staged");
        unpack(&clapp, &dest).expect("unpack");
        assert_eq!(
            fs::read_to_string(dest.join("clatch.json")).unwrap(),
            fs::read_to_string(src.join("clatch.json")).unwrap()
        );
        assert!(dest.join("bin/app").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            // write_app marks nothing +x; give the source one and repack to
            // prove modes survive.
            fs::set_permissions(src.join("bin/app"), fs::Permissions::from_mode(0o755)).unwrap();
            let clapp = pack(&src, &clatch_testkit::tmp()).unwrap();
            let dest = clatch_testkit::tmp().join("x");
            unpack(&clapp, &dest).unwrap();
            let mode = fs::metadata(dest.join("bin/app"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o111, 0o111, "exec bit survives: {mode:o}");
        }
    }

    #[test]
    fn unpack_rejects_escapes_and_manifestless_archives() {
        // An archive whose entry walks out of the target: loud, never partial-silent.
        let mut bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("../evil.txt", opts).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        let err = unpack_from(std::io::Cursor::new(bytes), &clatch_testkit::tmp()).unwrap_err();
        assert!(err.to_string().contains("escapes"), "{err}");

        // No root clatch.json: not a .clapp.
        let mut bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("readme.txt", opts).unwrap();
            zip.write_all(b"x").unwrap();
            zip.finish().unwrap();
        }
        let err = unpack_from(std::io::Cursor::new(bytes), &clatch_testkit::tmp()).unwrap_err();
        assert!(err.to_string().contains("clatch.json"), "{err}");
    }

    #[test]
    fn unpack_caps_the_uncompressed_payload() {
        // 64 KiB of zeros deflates to ~100 bytes: exactly the bomb shape. A
        // 1 KiB ceiling must kill it loudly; a fitting archive must pass.
        let mut bytes = Vec::new();
        {
            let mut zip = zip::ZipWriter::new(std::io::Cursor::new(&mut bytes));
            let opts = zip::write::SimpleFileOptions::default();
            zip.start_file("clatch.json", opts).unwrap();
            zip.write_all(&vec![0u8; 64 * 1024]).unwrap();
            zip.finish().unwrap();
        }
        let err = unpack_with_limit(
            std::io::Cursor::new(bytes.clone()),
            &clatch_testkit::tmp(),
            1024,
        )
        .unwrap_err();
        assert!(err.to_string().contains("ceiling"), "{err}");
        unpack_with_limit(
            std::io::Cursor::new(bytes),
            &clatch_testkit::tmp(),
            1024 * 1024,
        )
        .expect("a fitting archive passes the same gate");
    }

    #[test]
    fn github_spec_parses_the_documented_forms_only() {
        let s = |o: &str, r: &str, t: Option<&str>| GithubSpec {
            owner: o.into(),
            repo: r.into(),
            tag: t.map(String::from),
        };
        assert_eq!(
            parse_github("arfium/arfchess"),
            Some(s("arfium", "arfchess", None))
        );
        assert_eq!(
            parse_github("arfium/arfchess@v0.1.0"),
            Some(s("arfium", "arfchess", Some("v0.1.0")))
        );
        assert_eq!(
            parse_github("https://github.com/arfium/arfchess"),
            Some(s("arfium", "arfchess", None))
        );
        assert_eq!(
            parse_github("https://github.com/arfium/arfchess.git"),
            Some(s("arfium", "arfchess", None))
        );
        assert_eq!(
            parse_github("https://github.com/arfium/arfchess/releases/tag/v0.2.0"),
            Some(s("arfium", "arfchess", Some("v0.2.0")))
        );
        assert_eq!(
            parse_github("https://github.com/arfium/arfchess/releases/latest"),
            Some(s("arfium", "arfchess", None))
        );
        // Not repos: plain words, deep URLs, other hosts, existing paths,
        // and dot-only segments (path steps, never names).
        assert_eq!(parse_github("arfchess"), None);
        assert_eq!(parse_github("a/b/c"), None);
        assert_eq!(parse_github("../.."), None);
        assert_eq!(parse_github("a/.."), None);
        assert_eq!(parse_github("./a"), None);
        assert_eq!(parse_github("https://github.com/a/b/issues/1"), None);
        assert_eq!(parse_github("https://gitlab.com/a/b"), None);
        let dir = clatch_testkit::tmp();
        fs::create_dir_all(dir.join("a/b")).unwrap();
        let existing = dir.join("a/b");
        assert_eq!(parse_github(existing.to_str().unwrap()), None);
    }

    #[test]
    fn asset_pick_prefers_exact_then_any_then_single_crossplatform() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let all = names(&[
            "x-macos-arm64.clapp",
            "x-macos-x64.clapp",
            "x-linux-x64.clapp",
            "x-macos-arm64.clapp.sha256",
            "notes.txt",
        ]);
        assert_eq!(
            pick_asset(&all, "macos", "arm64").unwrap(),
            "x-macos-arm64.clapp"
        );
        let anyos = names(&["x-macos-any.clapp", "x-linux-x64.clapp"]);
        assert_eq!(
            pick_asset(&anyos, "macos", "arm64").unwrap(),
            "x-macos-any.clapp"
        );
        let cross = names(&["x.clapp", "notes.txt"]);
        assert_eq!(pick_asset(&cross, "macos", "arm64").unwrap(), "x.clapp");
        // Two markerless candidates are ambiguous; none at all is loud too.
        let ambiguous = names(&["x.clapp", "y.clapp"]);
        assert!(pick_asset(&ambiguous, "macos", "arm64").is_err());
        let none = names(&["x-windows-x64.clapp"]);
        let err = pick_asset(&none, "macos", "arm64").unwrap_err().to_string();
        assert!(
            err.contains("x-windows-x64.clapp"),
            "lists what ships: {err}"
        );
    }

    #[test]
    fn stage_rejects_a_non_source_spec_loudly() {
        let home = clatch_testkit::tmp();
        let err = stage(&home, "not-a-source", &|_| {})
            .unwrap_err()
            .to_string();
        assert!(err.contains("not an install source"), "{err}");
        // The staging dir it briefly created is torn down on the error path.
        let tmp = home.join("tmp");
        let leftovers = fs::read_dir(&tmp).map(|d| d.count()).unwrap_or(0);
        assert_eq!(leftovers, 0, "no staging leftovers");
    }
}
