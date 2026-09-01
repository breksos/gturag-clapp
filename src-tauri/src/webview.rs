//! Windows only: make sure there is a webview before Tauri goes looking for one.
//!
//! A Tauri app on Windows draws its window with **Microsoft Edge WebView2**, which is a
//! separate runtime from the app. Windows 11 ships it; Windows 10 usually has it because
//! Edge installs it; a fresh Server, LTSC or N image often does not. Without it, the raw
//! failure is a modal dialog from the WebView2 loader saying "Could not find the WebView2
//! Runtime" — no path, no link, and nothing in the terminal — which is a terrible way to
//! learn that a 5 MB download is missing.
//!
//! So this runs first, and there are three outcomes:
//!
//!   * a **vendored runtime** in `<install>/vendor/webview2/` → point WebView2 at it (the
//!     Fixed Version distribution, for machines that cannot install anything). Optional and
//!     absent by default: it is ~180 MB, which is thirty times the rest of the app.
//!   * an **installed runtime** → carry on, silently.
//!   * **neither** → print what is missing, where to get it, and exit. The agent's CLI half
//!     is a separate invocation and keeps working regardless: `gturag --help` and
//!     `gturag status` need no webview at all.
//!
//! No new dependency: the check is the two registry keys Microsoft documents for the
//! Evergreen runtime, read with `reg.exe`, plus the folder it installs into. It is a
//! no-op on macOS and Linux, whose webviews ship with the OS.
//!
//! (This belongs in clappkit — every Tauri clapp needs it. It lives here until it is moved,
//! so that the first Windows release is not the one that discovers the problem.)

/// The Evergreen runtime's client id under EdgeUpdate — Microsoft's documented detection
/// key, machine-wide and per-user.
#[cfg(windows)]
const CLIENT_ID: &str = "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}";

/// Where Microsoft's own installer puts the Evergreen runtime.
#[cfg(windows)]
const INSTALL_DIRS: &[&str] = &[
    r"C:\Program Files (x86)\Microsoft\EdgeWebView\Application",
    r"C:\Program Files\Microsoft\EdgeWebView\Application",
];

/// The Evergreen Bootstrapper — a ~2 MB download that installs the runtime.
pub const DOWNLOAD_URL: &str = "https://go.microsoft.com/fwlink/p/?LinkId=2124703";

/// Check for a webview, or explain and exit. Call before Tauri starts.
#[cfg(windows)]
pub fn ensure(cli: &str) {
    if let Some(dir) = vendored() {
        // The Fixed Version distribution: the loader reads this variable and uses that copy
        // instead of the installed one.
        std::env::set_var("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", &dir);
        eprintln!("{cli}: using the WebView2 runtime vendored at {}", dir.display());
        return;
    }
    if installed() {
        return;
    }
    eprintln!(
        "{cli}: this app draws its window with the Microsoft Edge WebView2 Runtime, which is \
         not installed on this machine.\n\
         \n\
         Install it (about 2 MB, from Microsoft):\n\
             {DOWNLOAD_URL}\n\
         \n\
         Or, for a machine that cannot install anything, unpack Microsoft's WebView2 Fixed \
         Version distribution into:\n\
             <install>\\vendor\\webview2\\\n\
         \n\
         The command line half needs none of this: `{cli} --help` works already."
    );
    std::process::exit(1);
}

#[cfg(not(windows))]
pub fn ensure(_cli: &str) {}

/// A Fixed Version runtime unpacked beside the app, if there is one. The folder must hold
/// `msedgewebview2.exe` — an empty `vendor/webview2/` is a mistake, not a runtime.
#[cfg(windows)]
fn vendored() -> Option<std::path::PathBuf> {
    let dir = clappkit::paths::install_root().join("vendor").join("webview2");
    dir.join("msedgewebview2.exe").is_file().then_some(dir)
}

/// Is the Evergreen runtime installed, for this user or for the machine?
#[cfg(windows)]
fn installed() -> bool {
    if INSTALL_DIRS.iter().any(|d| std::path::Path::new(d).is_dir()) {
        return true;
    }
    let keys = [
        format!(r"HKLM\SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\{CLIENT_ID}"),
        format!(r"HKLM\SOFTWARE\Microsoft\EdgeUpdate\Clients\{CLIENT_ID}"),
        format!(r"HKCU\SOFTWARE\Microsoft\EdgeUpdate\Clients\{CLIENT_ID}"),
    ];
    keys.iter().any(|k| has_version(k))
}

/// `reg query <key> /v pv` — and a version of "0.0.0.0" means the runtime was uninstalled
/// but left its key behind, which Microsoft's own detection guidance calls "not installed".
#[cfg(windows)]
fn has_version(key: &str) -> bool {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let out = std::process::Command::new("reg")
        .args(["query", key, "/v", "pv"])
        .creation_flags(CREATE_NO_WINDOW)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            let text = String::from_utf8_lossy(&o.stdout);
            match text.split_whitespace().last() {
                Some(v) => !v.is_empty() && v != "0.0.0.0",
                None => false,
            }
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn the_download_link_is_microsofts_own_bootstrapper() {
        assert!(super::DOWNLOAD_URL.starts_with("https://go.microsoft.com/"));
    }

    /// On macOS and Linux this must be a no-op that cannot fail: their webviews ship with
    /// the OS, and an app that refused to start over a Windows runtime would be absurd.
    #[test]
    fn the_check_is_a_no_op_off_windows() {
        #[cfg(not(windows))]
        super::ensure("gturag");
    }
}
