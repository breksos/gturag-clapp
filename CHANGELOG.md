# Changelog

[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Pre-1.0, so a minor bump may
break: SemVer 0.x rules. Versions before 0.1.5 were not recorded here; their notes are the
GitHub releases.

## [0.1.5] - 2026-09-14

### Changed
- clappkit moves to its current main (`2cde169`), which carries the K1-K5 security fixes.
  The one that reached this app's code is the avatar bridge: `asset` now serves only the
  avatar paths the agent roster names, through `clappkit::app::avatar_uri`, instead of any
  file the user can read.
- Build, packaging and verification are the family's: `scripts/lib.sh`, `build.sh`,
  `validate.sh`, `pack.sh`, `.github/smoke.sh`, and the shared bodies of `package.sh` and
  `verify.sh` are byte-identical to template-clapp. The npm scripts carry the family's names.
- Every depot names exactly one launch OS, its own, on all three platforms.
- The frontend builds to `dist-web/`, the name every clapp uses.
- `gturag -h` says where the model really goes: this app's data directory, or the shared
  store when a copy is already there. It used to promise a shared cache the app never writes.

### Added
- `connector.cliBin`, explicit, as every clapp declares it.
- `npm run check` (`scripts/check-manifest.mjs`): the manifest against the code, the version
  in all four places, the release matrix against the launch keys, and the icon and banner
  against the format's bounds.
- A manual-only CI workflow, and a release workflow on the template's shape that still
  ships Linux.
- AGENTS.md, CLAUDE.md, LICENSE (Apache-2.0, as Cargo.toml already declared),
  .editorconfig, and this file.
