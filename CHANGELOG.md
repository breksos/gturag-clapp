# Changelog

[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Pre-1.0, so a minor bump may
break: SemVer 0.x rules. Versions before 0.1.5 were not recorded here; their notes are the
GitHub releases.

## [0.1.6] - 2026-09-17

The fixes a graduate student's week with the app asked for.

### Fixed
- `get` works for every document whose code has a Turkish letter. The id was filtered to
  ASCII before it was fetched, so `İA-0021` asked for `A-0021`, and every İA, YÖ and KİDR
  document had no full text.
- `IA-0021`, `ia-0021`, `YO-0054` and `yö-54` find their documents: codes are compared with
  Turkish letters folded to ASCII, and a full id is found however it is cased.
- A document's revision is the one it prints about itself. `İA-0021` said R0, from a file
  name with no revision in it; its own header says revision 1 of 10 June 2024.
- `sync` gives a definitive answer — up to date with both dates, updated, or could not check
  — instead of "ready". It no longer reloads the index it already has, so the results on
  screen stay, and a newer index re-runs the query on screen.
- The first search of a session no longer waits twenty seconds: the model is warmed while
  the window says it is loading.
- Saving a saved document says it was already in the list.
- A long question no longer ranks worse than a short one: filler words are dropped, and the
  education level it names is a preference, so `yüksek lisansta tez danışmanımı değiştirmek
  istiyorum` puts the graduate advisor-change form first, and `staj başvurusu` finds the
  internship documents rather than every other application.
- XML entities (`&amp;`) no longer reach the text of 153 documents, and a Word hyperlink
  keeps its target.

### Added
- A freshness check against the university's site when a document is opened or fetched: an
  out-of-date copy says so, with the current file's address.
- Each result's collection, revision and date, unit and level, in both surfaces; `open`
  prints where the document matched.
- A note above the results when a question is outside the archive (calendars, meeting days,
  announcements) or nothing matches well.
- English questions are matched through a glossary of this domain, and say so.
- `open`, `get`, `save` and `unsave` take the row number of a result on screen.
- Filters by collection, level and language: `search --type/--level/--lang/--all`, and in
  the window's toolbar.
- `get` prints Markdown: a header of facts, then the text without page furniture.

### Archive
- Refreshed from the university's site on 17 September 2026: 1851 documents, 9104 passages.
  YÖ-0054 is revision 8 of 10.09.2026; 18 documents are new (among them YÖ-0003 and
  YÖ-0066); the 17 the university no longer lists are gone; six survey documents have their
  text back.
- The pipeline no longer loses documents to three things it used to mistake for "not
  published": a register revision the university has since replaced (the probe now looks a
  few revisions ahead), a file the server stores under a decomposed name (the download
  retries it, and records the address that worked), and a request that timed out (the
  document is kept, not culled).

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
