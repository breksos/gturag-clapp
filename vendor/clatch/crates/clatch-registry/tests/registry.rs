//! install → list → uninstall, with per-app settings preserved (Steam behavior),
//! and invalid-manifest rejection.

use clatch_core::AppId;
use clatch_registry::Registry;
use clatch_testkit::tmp;
use std::fs;

const MANIFEST: &str = r#"{
  "manifestVersion": 1,
  "id": "com.arfium.arfchess",
  "name": "ArfChess",
  "description": "Play chess against your agent.",
  "version": "0.1.0",
  "protocol": 2,
  "icon": "assets/icon.png",
  "launch": { "linux": "bin/arfchess-app", "macos": "bin/arfchess-app", "windows": "bin/arfchess-app" },
  "connector": { "cli": "arfchess", "signals": [ {"id":"move","type":"run"}, {"id":"game.over","type":"context"} ] }
}"#;

#[test]
fn install_list_uninstall_preserves_settings() {
    let home = tmp();
    let src = tmp();
    fs::write(src.join("clatch.json"), MANIFEST).unwrap();
    fs::create_dir_all(src.join("bin")).unwrap();
    fs::write(src.join("bin/arfchess-app"), "#!/bin/sh\n").unwrap(); // launch
    fs::write(src.join("bin/arfchess"), "#!/bin/sh\n").unwrap(); // agent CLI
    fs::create_dir_all(src.join("assets")).unwrap();
    fs::write(src.join("assets/icon.png"), "png").unwrap();

    let reg = Registry::new(home.clone());
    let id = AppId::new("com.arfium.arfchess");

    let rec = reg.install(&src).unwrap();
    assert_eq!(rec.id, id);
    assert_eq!(rec.cli.as_deref(), Some("arfchess"));
    assert_eq!(rec.icon.as_deref(), Some("assets/icon.png"));
    // Typed declarations survive the record round-trip (signals.md): the name
    // AND the manifest-fixed type.
    let sigs: Vec<(&str, clatch_core::SignalType)> = rec
        .signals
        .iter()
        .map(|s| (s.id.as_str(), s.signal_type))
        .collect();
    assert_eq!(
        sigs,
        vec![
            ("move", clatch_core::SignalType::Run),
            ("game.over", clatch_core::SignalType::Context),
        ]
    );
    assert!(rec.source.starts_with("local:"));
    assert!(rec.install_dir.join("clatch.json").exists());
    assert!(rec.install_dir.join("bin/arfchess-app").exists());

    assert_eq!(reg.list().unwrap().len(), 1);
    assert!(reg.get(&id).unwrap().is_some());

    // per-app settings must survive uninstall (Steam behavior)
    let settings_file = home.join("settings").join("com.arfium.arfchess.json");
    fs::create_dir_all(settings_file.parent().unwrap()).unwrap();
    fs::write(&settings_file, "{}").unwrap();

    reg.uninstall(&id, false).unwrap();
    assert!(reg.get(&id).unwrap().is_none());
    assert!(!rec.install_dir.exists());
    assert!(
        settings_file.exists(),
        "settings must be preserved on uninstall"
    );
}

#[test]
fn the_element_type_matrix_is_enforced_loudly() {
    // reference/elements.md: each type has required and FORBIDDEN surfaces;
    // forbidden is rejected at validate, never ignored.
    let parse = |json: &str| clatch_registry::Manifest::parse(json).expect("parse");
    let base = |ty: &str, extra: &str| {
        format!(
            r#"{{ "manifestVersion": 1, "type": "{ty}", "id": "com.x.e", "name": "E",
                 "description": "d", "version": "1"{extra} }}"#
        )
    };

    // cli: valid minimal; then each forbidden surface rejects.
    assert!(parse(&base("cli", r#", "connector": {"cli": "e"}"#))
        .validate()
        .is_ok());
    for (label, extra) in [
        (
            "launch",
            r#", "launch": {"linux": "bin/x"}, "connector": {"cli": "e"}"#,
        ),
        ("protocol", r#", "protocol": 2, "connector": {"cli": "e"}"#),
        (
            "signals",
            r#", "connector": {"cli": "e", "signals": [{"id": "s", "type": "run"}]}"#,
        ),
        ("no cli", r#", "connector": {}"#),
    ] {
        let e = parse(&base("cli", extra))
            .validate()
            .unwrap_err()
            .to_string();
        assert!(e.contains("cli element"), "{label}: {e}");
    }

    // skill: valid minimal; then each forbidden surface rejects.
    assert!(parse(&base("skill", "")).validate().is_ok());
    for (label, extra) in [
        ("launch", r#", "launch": {"linux": "bin/x"}"#),
        ("protocol", r#", "protocol": 2"#),
        ("cli", r#", "connector": {"cli": "e"}"#),
        ("login", r#", "connector": {"login": "auth login"}"#),
    ] {
        let e = parse(&base("skill", extra))
            .validate()
            .unwrap_err()
            .to_string();
        assert!(e.contains("skill element"), "{label}: {e}");
    }

    // clapp: login is forbidden (its GUI owns auth); an untyped manifest IS a
    // clapp (the default that keeps every existing package valid).
    let clapp = r#"{ "manifestVersion": 1, "id": "com.x.e", "name": "E", "description": "d",
                    "version": "1", "protocol": 2, "launch": {"linux": "bin/x"},
                    "connector": {"cli": "e", "login": "auth login"} }"#;
    let e = parse(clapp).validate().unwrap_err().to_string();
    assert!(e.contains("clapp element") && e.contains("login"), "{e}");
}

#[test]
fn cli_and_skill_elements_install_by_their_own_rules() {
    let home = tmp();
    let reg = Registry::new(home.clone());

    // A cli element installs, links its CLI, records its type + login verbs.
    let src = tmp();
    clatch_testkit::write_cli_element(&src, "toolx", Some(("auth login", "auth status")));
    let rec = reg.install(&src).unwrap();
    assert_eq!(rec.element_type, clatch_core::ElementType::Cli);
    assert_eq!(rec.cli.as_deref(), Some("toolx"));
    assert_eq!(rec.login.as_deref(), Some("auth login"));
    assert_eq!(rec.login_check.as_deref(), Some("auth status"));
    // The shim's NAME is the shared rule's answer, never spelled by hand: on
    // Windows it carries `.cmd`, and this assertion said `toolx` flat, so it
    // failed there from the day it was written - unseen, because CI could not
    // build at all (2026-08-01).
    assert!(
        clatch_core::shim::entry(&home.join("bin"), "toolx").exists(),
        "cli linked onto PATH"
    );

    // A skill element installs with no CLI link at all.
    let src = tmp();
    clatch_testkit::write_skill_element(&src, "com.x.knowhow");
    let rec = reg.install(&src).unwrap();
    assert_eq!(rec.element_type, clatch_core::ElementType::Skill);
    assert!(rec.cli.is_none());

    // A skill without SKILL.md is rejected at the file gate.
    let src = tmp();
    clatch_testkit::write_skill_element(&src, "com.x.empty");
    std::fs::remove_file(src.join("SKILL.md")).unwrap();
    let e = Registry::new(tmp()).install(&src).unwrap_err().to_string();
    assert!(e.contains("SKILL.md"), "{e}");
}

#[test]
fn recover_installs_finishes_or_undoes_a_crashed_reinstall() {
    let home = tmp();
    let reg = Registry::new(home.clone());
    let apps = home.join("apps");
    let registry = home.join("registry");
    fs::create_dir_all(&apps).unwrap();
    fs::create_dir_all(&registry).unwrap();
    let content = |dir: &std::path::Path, marker: &str| {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("marker"), marker).unwrap();
    };

    // A - crash AFTER the content swap, BEFORE the commit: new content is live,
    // the staged record is the journal, the old record still stands, and the
    // staging dir is GONE (consumed by the swap). Recovery ROLLS FORWARD.
    let a = "com.x.fwd";
    content(&apps.join(a), "v2"); // new content, live
    content(&apps.join(format!("{a}.outgoing")), "v1"); // old aside
    fs::write(registry.join(format!("{a}.json")), "OLD").unwrap();
    fs::write(registry.join(format!("{a}.json.incoming")), "NEW").unwrap();

    // B - crash DURING the swap (new content still staged, old still live):
    // recovery ROLLS BACK, keeping the old app.
    let b = "com.x.back";
    content(&apps.join(b), "v1"); // old content, live
    content(&apps.join(format!("{b}.incoming")), "v2"); // new content, staged
    fs::write(registry.join(format!("{b}.json")), "OLD-B").unwrap();
    fs::write(registry.join(format!("{b}.json.incoming")), "NEW-B").unwrap();

    // C - crash BETWEEN moving the old dir aside and swapping the new in (dest
    // gone, staging present): ROLL BACK by restoring `.outgoing`.
    let c = "com.x.mid";
    content(&apps.join(format!("{c}.incoming")), "v2c"); // new, staged
    content(&apps.join(format!("{c}.outgoing")), "v1c"); // old, aside; no dest
    fs::write(registry.join(format!("{c}.json.incoming")), "NEW-C").unwrap();

    reg.recover_installs();

    // A rolled forward: journal became the record; `.incoming`/`.outgoing` gone.
    assert_eq!(
        fs::read_to_string(registry.join(format!("{a}.json"))).unwrap(),
        "NEW"
    );
    assert!(!registry.join(format!("{a}.json.incoming")).exists());
    assert!(!apps.join(format!("{a}.outgoing")).exists());
    assert_eq!(
        fs::read_to_string(apps.join(a).join("marker")).unwrap(),
        "v2"
    );

    // B rolled back: journal + staging gone; old app and record untouched.
    assert!(!registry.join(format!("{b}.json.incoming")).exists());
    assert!(!apps.join(format!("{b}.incoming")).exists());
    assert_eq!(
        fs::read_to_string(registry.join(format!("{b}.json"))).unwrap(),
        "OLD-B"
    );
    assert_eq!(
        fs::read_to_string(apps.join(b).join("marker")).unwrap(),
        "v1"
    );

    // C rolled back: dest restored from `.outgoing`; journal + staging gone; and
    // NO record was committed (the reinstall never happened).
    assert_eq!(
        fs::read_to_string(apps.join(c).join("marker")).unwrap(),
        "v1c"
    );
    assert!(!apps.join(format!("{c}.incoming")).exists());
    assert!(!apps.join(format!("{c}.outgoing")).exists());
    assert!(!registry.join(format!("{c}.json.incoming")).exists());
    assert!(!registry.join(format!("{c}.json")).exists());
}

#[test]
fn rejects_a_package_missing_a_declared_file() {
    // A `dist` that declares an icon (or a host-OS launch binary) but does not
    // ship it must be rejected at install, not installed broken with a dangling
    // CLI link (the real arfchess failure: `command not found` at run time).
    let src = tmp();
    fs::write(src.join("clatch.json"), MANIFEST).unwrap();
    fs::create_dir_all(src.join("bin")).unwrap();
    fs::write(src.join("bin/arfchess-app"), "#!/bin/sh\n").unwrap();
    // Note: assets/icon.png is DECLARED but not created.
    let err = Registry::new(tmp()).install(&src).unwrap_err();
    assert!(
        format!("{err}").contains("icon not found"),
        "a missing declared icon is rejected: {err}"
    );
}

#[test]
fn rejects_a_package_missing_its_declared_cli_binary() {
    // The arfchess failure: a manifest declares a CLI but the package has no
    // binary for it - install must reject, not leave a dangling link.
    let src = tmp();
    fs::write(src.join("clatch.json"), MANIFEST).unwrap();
    fs::create_dir_all(src.join("assets")).unwrap();
    fs::write(src.join("assets/icon.png"), "png").unwrap();
    fs::create_dir_all(src.join("bin")).unwrap();
    fs::write(src.join("bin/arfchess-app"), "#!/bin/sh\n").unwrap();
    // The manifest's `connector.cli` is "arfchess" -> cli_bin "bin/arfchess", not created.
    let err = Registry::new(tmp()).install(&src).unwrap_err();
    assert!(
        format!("{err}").contains("CLI binary not found"),
        "a missing declared CLI binary is rejected: {err}"
    );
}

#[test]
fn rejects_invalid_manifest() {
    let src = tmp();
    fs::write(
        src.join("clatch.json"),
        r#"{ "manifestVersion": 1, "id": "", "name": "x", "description": "",
            "version": "0", "protocol": 2, "launch": {}, "connector": { "cli": "" } }"#,
    )
    .unwrap();
    assert!(Registry::new(tmp()).install(&src).is_err());
}

#[cfg(unix)]
#[test]
fn install_links_the_cli_onto_the_bin_dir_and_uninstall_removes_it() {
    let home = tmp();
    let src = tmp();
    // cli `arf`, binary at the bin/<cli> default.
    fs::write(
        src.join("clatch.json"),
        r#"{ "manifestVersion":1, "id":"com.x.arf", "name":"Arf", "description":"d",
             "version":"0.1.0", "protocol":2, "launch":{ "linux":"bin/arf", "macos":"bin/arf", "windows":"bin/arf" },
             "connector":{ "cli":"arf", "signals":[] } }"#,
    )
    .unwrap();
    fs::create_dir_all(src.join("bin")).unwrap();
    fs::write(src.join("bin/arf"), "#!/bin/sh\n").unwrap();

    let reg = Registry::new(home.clone());
    let id = AppId::new("com.x.arf");
    reg.install(&src).unwrap();

    // The shim exists in <home>/bin as the env-injecting exec wrapper
    // (reference/elements.md § cli): it names its target inside the install
    // dir AND exports the app's CLATCH_DATA_DIR before exec.
    let shim = home.join("bin").join("arf");
    assert!(shim.exists(), "cli shim linked");
    let body = fs::read_to_string(&shim).unwrap();
    assert!(
        body.contains(&home.join("apps").join("com.x.arf").display().to_string()),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            "CLATCH_DATA_DIR=\"{}\"",
            home.join("appdata").join("com.x.arf").display()
        )),
        "{body}"
    );

    // A second app claiming the same shorthand is rejected.
    let src2 = tmp();
    fs::write(
        src2.join("clatch.json"),
        r#"{ "manifestVersion":1, "id":"com.x.other", "name":"Other", "description":"d",
             "version":"0.1.0", "protocol":2, "launch":{ "linux":"bin/arf", "macos":"bin/arf", "windows":"bin/arf" },
             "connector":{ "cli":"arf", "signals":[] } }"#,
    )
    .unwrap();
    fs::create_dir_all(src2.join("bin")).unwrap();
    fs::write(src2.join("bin/arf"), "#!/bin/sh\n").unwrap();
    assert!(
        reg.install(&src2).is_err(),
        "a clashing cli shorthand is refused"
    );

    // Uninstall removes the shim.
    reg.uninstall(&id, false).unwrap();
    assert!(!shim.exists(), "cli shim removed on uninstall");
}

#[test]
fn a_signalless_clapp_installs_but_a_cli_less_one_is_rejected() {
    // tools.md § Connectors (2026-07-18): the CLI is the clapp's constant
    // surface (`-h` is the floor), so a manifest without one is rejected at
    // install; an EMPTY signal set is still a perfectly good clapp (optional
    // facets never fork the class). Supersedes the earlier cli-optional delta.
    let home = tmp();
    let src = tmp();
    fs::write(
        src.join("clatch.json"),
        r#"{ "manifestVersion": 1, "id": "com.x.headless", "name": "Headless",
            "description": "no cli, no signals", "version": "0.1.0", "protocol": 2,
            "launch": { "linux": "bin/app", "macos": "bin/app", "windows": "bin/app" } }"#,
    )
    .unwrap();
    fs::create_dir_all(src.join("bin")).unwrap();
    fs::write(src.join("bin/app"), "#!/bin/sh\n").unwrap();

    let registry = Registry::new(home);
    let err = registry.install(&src).unwrap_err().to_string();
    assert!(err.contains("connector.cli"), "{err}");

    // Declare the CLI (its binary already ships) and the same package installs.
    fs::write(
        src.join("clatch.json"),
        r#"{ "manifestVersion": 1, "id": "com.x.headless", "name": "Headless",
            "description": "cli, no signals", "version": "0.1.0", "protocol": 2,
            "launch": { "linux": "bin/app", "macos": "bin/app", "windows": "bin/app" },
            "connector": { "cli": "headless", "cliBin": "bin/app", "signals": [] } }"#,
    )
    .unwrap();
    let rec = registry.install(&src).unwrap();
    assert_eq!(rec.cli.as_deref(), Some("headless"));
    assert!(rec.signals.is_empty(), "a signalless clapp is a clapp");
}

#[test]
fn legacy_string_signal_records_still_load() {
    // A pre-Clapp-v1 home: the on-disk record (Clatch's own cache of the
    // manifest at install time) declares signals as bare name strings. It must
    // keep loading, adopted as `context` (the fail-safe type), or every client
    // command bricks on an upgraded daemon (field incident, 2026-07-18).
    let home = tmp();
    fs::create_dir_all(home.join("registry")).unwrap();
    fs::write(
        home.join("registry/com.x.legacy.json"),
        r#"{
          "schemaVersion": 1, "id": "com.x.legacy", "name": "Legacy",
          "description": "installed before typed signals", "cli": "legacy",
          "signals": ["move", "game.over"],
          "version": "0.1.0",
          "installDir": "/tmp/none", "installedAt": "2026-07-15T15:00:14Z",
          "source": "local:/tmp/none", "state": "installed"
        }"#,
    )
    .unwrap();

    let registry = Registry::new(home);
    let rec = registry
        .get(&AppId::new("com.x.legacy"))
        .unwrap()
        .expect("the legacy record loads");
    let sigs: Vec<(&str, clatch_core::SignalType)> = rec
        .signals
        .iter()
        .map(|s| (s.id.as_str(), s.signal_type))
        .collect();
    assert_eq!(
        sigs,
        vec![
            ("move", clatch_core::SignalType::Context),
            ("game.over", clatch_core::SignalType::Context),
        ],
        "bare strings adopt context, never run"
    );
}

#[cfg(unix)]
#[test]
fn a_reinstall_that_renames_its_cli_drops_the_old_shim() {
    let home = tmp();
    let manifest = |cli: &str| {
        format!(
            r#"{{ "manifestVersion":1, "id":"com.x.ren", "name":"Ren", "description":"d",
                 "version":"0.1.0", "protocol":2,
                 "launch":{{ "linux":"bin/{cli}", "macos":"bin/{cli}", "windows":"bin/{cli}" }},
                 "connector":{{ "cli":"{cli}", "signals":[] }} }}"#
        )
    };
    let src = tmp();
    fs::write(src.join("clatch.json"), manifest("foo")).unwrap();
    fs::create_dir_all(src.join("bin")).unwrap();
    fs::write(src.join("bin/foo"), "#!/bin/sh\n").unwrap();
    let reg = Registry::new(home.clone());
    reg.install(&src).unwrap();
    assert!(home.join("bin/foo").exists(), "first cli linked");

    // The same id comes back with a RENAMED cli: the superseded shim must not
    // stay behind, where it would dangle after uninstall and squat the name
    // against a later app that legitimately claims it.
    let src2 = tmp();
    fs::write(src2.join("clatch.json"), manifest("bar")).unwrap();
    fs::create_dir_all(src2.join("bin")).unwrap();
    fs::write(src2.join("bin/bar"), "#!/bin/sh\n").unwrap();
    reg.install(&src2).unwrap();
    assert!(home.join("bin/bar").exists(), "renamed cli linked");
    assert!(!home.join("bin/foo").exists(), "superseded shim dropped");
}

#[test]
fn an_app_cannot_claim_a_backend_launcher_name() {
    let home = tmp();
    // A backend-manager launcher already holds the name. Its entry is not in
    // registry's shim shape (so the parse sees no owner), but its body embeds
    // the <home>/backends dir - the same marker backends::owns() keys on.
    fs::create_dir_all(home.join("bin")).unwrap();
    let entry = if cfg!(windows) {
        "claude.cmd"
    } else {
        "claude"
    };
    fs::write(
        home.join("bin").join(entry),
        format!(
            "@ECHO off\r\nendLocal & \"{}\" %*\r\n",
            home.join("backends").join("node").display()
        ),
    )
    .unwrap();

    let src = tmp();
    fs::write(
        src.join("clatch.json"),
        r#"{ "manifestVersion":1, "id":"com.x.squat", "name":"Squat", "description":"d",
             "version":"0.1.0", "protocol":2,
             "launch":{ "linux":"bin/claude", "macos":"bin/claude", "windows":"bin/claude" },
             "connector":{ "cli":"claude", "signals":[] } }"#,
    )
    .unwrap();
    fs::create_dir_all(src.join("bin")).unwrap();
    fs::write(src.join("bin/claude"), "#!/bin/sh\n").unwrap();

    let err = Registry::new(home.clone()).install(&src).unwrap_err();
    assert!(
        format!("{err}").contains("reserved by an installed backend"),
        "a backend launcher is never clobbered: {err}"
    );
}

#[test]
fn purge_erases_the_apps_whole_footprint() {
    let home = tmp();
    let src = tmp();
    clatch_testkit::write_app(&src, "com.x.purged");
    let reg = Registry::new(home.clone());
    let id = AppId::new("com.x.purged");
    reg.install(&src).unwrap();
    // The kept-footprint trio, as the daemon and the app would have written it.
    fs::create_dir_all(home.join("settings")).unwrap();
    fs::write(home.join("settings/com.x.purged.json"), "{}").unwrap();
    fs::create_dir_all(home.join("stats")).unwrap();
    fs::write(home.join("stats/com.x.purged.json"), "{}").unwrap();
    fs::create_dir_all(reg.data_dir(&id)).unwrap();
    fs::write(reg.data_dir(&id).join("save.dat"), "x").unwrap();

    // Plain uninstall keeps all three (the Steam-save default)...
    reg.uninstall(&id, false).unwrap();
    assert!(home.join("settings/com.x.purged.json").exists());
    assert!(home.join("stats/com.x.purged.json").exists());
    assert!(reg.data_dir(&id).exists());

    // ...and purge erases them, even when the app is already uninstalled.
    reg.uninstall(&id, true).unwrap();
    assert!(!home.join("settings/com.x.purged.json").exists());
    assert!(!home.join("stats/com.x.purged.json").exists());
    assert!(!reg.data_dir(&id).exists());
}
