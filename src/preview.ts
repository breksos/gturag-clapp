// Render the window in a plain browser, against a fake snapshot.
//
// This is how you look at the UI without a build, and — more usefully — how you check the
// states that are awkward to reach on demand: mid-provisioning, provisioning failed, an
// agent-driven search, a title-only document. Waiting for a 450 MB download to see what
// the progress bar looks like is not a workflow.
//
//     npm run dev:web        then open /?preview=<state>
//
// It works by answering the two calls bridge.ts makes — `run_cmd` and the `state` event —
// before @tauri-apps/api can fail to find a host. Nothing in App.tsx knows it is being
// previewed, which is the point: what you see is the real component tree.

import type { Doc, Snapshot, Stage } from "./bridge";
import { EMPTY } from "./bridge";

function doc(
  code: string,
  title: string,
  extra: Partial<Doc> = {},
): Doc {
  return {
    id: `${code}.tr`,
    code,
    rev: 1,
    lang: "tr",
    title,
    name: `${code} ${title} R1.docx`,
    ext: "docx",
    url: `https://www.gtu.edu.tr/fileman/${code}.docx`,
    titleOnly: false,
    saved: false,
    score: 0.03,
    why: "match",
    snippet: `${title}\nBu form, ilgili birime elden ya da e-posta ile iletilir. İki nüsha doldurulur.`,
    passages: [
      `${title}\nBu form, ilgili birime elden ya da e-posta ile iletilir. İki nüsha doldurulur.`,
      `${title}\nDanışman değişikliği, öğrencinin ve/veya danışmanın başvurusu üzerine yapılabilir.`,
    ],
    ...extra,
  };
}

const RESULTS: Doc[] = [
  doc("FR-0083", "YL-DR Danışman Değişikliği Formu", { why: "code", score: 1 }),
  doc("FR-0086", "YL-DR Farklı Üniversiteden Ders Alma Bildirim Formu"),
  doc("FR-0087", "YL-DR Mazeretli Kayıt Formu", { saved: true }),
  doc("FR-0336", "Staj Belgesi", { titleOnly: true, snippet: "Staj Belgesi", ext: "doc" }),
  doc("FR-0175", "Lisans-Lisansüstü İlişik Kesme Formu", { lang: "tr", rev: 7 }),
];

const AGENTS = [
  { id: "a-1", name: "Berk", backend: "claude-code", model: "opus", avatar: null },
  { id: "a-2", name: "Deniz", backend: "claude-code", model: null, avatar: null },
];

const BASE: Snapshot = {
  ...EMPTY,
  rev: 1,
  query: "danışman değiştirmek istiyorum",
  // What `index::tokenize` produces for that query: every word AND its 5-character stem.
  // Highlighting must be checked against the real shape, or the preview would show marks
  // the running app never draws (and miss the stem matches it does).
  terms: ["danışman", "danış", "değiştirmek", "değiş", "istiyorum", "istiy"],
  title: "“danışman değiştirmek istiyorum” — 5 sonuç",
  searchedBy: "a-1",
  searchedByName: "Berk",
  results: RESULTS,
  total: RESULTS.length,
  open: RESULTS[0],
  saved: [RESULTS[2]],
  provision: { model: { stage: "ready" }, index: { stage: "ready" }, ready: true, summary: "ready" },
  app: { name: "GTÜ Formlar" },
  corpus: {
    documents: 791,
    chunks: 2338,
    built: "2026-08-12",
    source: "https://www.gtu.edu.tr/kategori/2382/0/display.aspx",
    updateUrl: null,
  },
  agents: AGENTS,
};

function stage(model: Stage, index: Stage, summary: string): Snapshot {
  return {
    ...BASE,
    query: "",
    results: [],
    total: 0,
    open: null,
    saved: [],
    provision: { model, index, ready: false, summary },
  };
}

/** Every state worth looking at, by name. */
const STATES: Record<string, Snapshot> = {
  default: BASE,
  empty: { ...BASE, query: "", results: [], total: 0, open: null, saved: [] },
  "no-results": { ...BASE, query: "qwertyuiop", results: [], total: 0, open: null },
  // The regression: an agent ran `open` with no search behind it. Query empty, results
  // empty, but a form IS open — the window must show it rather than the welcome hero.
  "agent-opened": {
    ...BASE,
    query: "",
    title: "",
    searchedBy: null,
    searchedByName: null,
    terms: [],
    results: [],
    total: 0,
    open: RESULTS[0],
    saved: [],
    activity: [
      { seq: 1, who: "a-1", whoName: "Berk", action: "open", detail: "FR-0083 YL-DR Danışman Değişikliği Formu" },
    ],
  },
  downloading: stage(
    { stage: "downloading", percent: 37 },
    { stage: "ready" },
    "downloading the model — 37%",
  ),
  "index-downloading": stage(
    { stage: "missing" },
    { stage: "downloading", percent: 68 },
    "downloading the index — 68%",
  ),
  failed: stage(
    { stage: "missing" },
    { stage: "failed", reason: "not a GTÜ Formlar index (bad magic)" },
    "provisioning failed: not a GTÜ Formlar index (bad magic)",
  ),
};

export function installPreview(): boolean {
  const name = new URLSearchParams(location.search).get("preview");
  if (name === null) return false;

  const snapshot = STATES[name] ?? STATES.default;
  let current = { ...snapshot };

  // Only `invoke` is stubbed. `listen` is left to fail — useSnapshot already swallows that
  // (`.catch(() => {})`) and applies the invoke *response*, which is the same snapshot, so
  // the UI updates on every action anyway. What preview therefore does NOT exercise is a
  // core-initiated push (provisioning progress arriving on its own); those states are
  // reachable here as named snapshots instead, which is the part worth looking at.
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    invoke: async (cmd: string, args: Record<string, unknown>) => {
      if (cmd === "asset") return null;
      // Preview runs in a real browser, where target="_blank" works — so the link can
      // simply open, and clicking it here proves the wiring rather than doing nothing.
      if (cmd === "open_url") {
        window.open(String(args?.url ?? ""), "_blank", "noreferrer");
        return null;
      }
      const req = (args?.req ?? {}) as { cmd?: string; query?: string; id?: string; by?: string };
      // Enough behaviour to click around: searching filters, opening selects, saving toggles.
      if (req.cmd === "search") {
        const q = (req.query ?? "").toLocaleLowerCase("tr");
        const hits = q ? RESULTS.filter((d) => `${d.code} ${d.title}`.toLocaleLowerCase("tr").includes(q)) : [];
        current = { ...current, rev: current.rev! + 1, query: req.query ?? "", results: hits, total: hits.length };
      } else if (req.cmd === "open") {
        current = { ...current, rev: current.rev! + 1, open: RESULTS.find((d) => d.id === req.id) ?? null };
      } else if (req.cmd === "save" || req.cmd === "unsave") {
        const on = req.cmd === "save";
        const flip = (d: Doc) => (d.id === req.id ? { ...d, saved: on } : d);
        const results = current.results.map(flip);
        current = {
          ...current,
          rev: current.rev! + 1,
          results,
          saved: results.filter((d) => d.saved),
        };
      } else if (req.cmd === "sort") {
        current = { ...current, rev: current.rev! + 1, sort: (req as { by: Snapshot["sort"] }).by };
      }
      return current;
    },
  };

  document.title = `GTÜ Formlar — preview: ${name}`;
  return true;
}
