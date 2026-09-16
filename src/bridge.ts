// The one seam between the window and the core. Nothing else in src/ imports @clappkit,
// and nothing else calls `invoke` — so there is exactly one place where the shape of a
// snapshot is written down, and one place to look when it changes.

export { useAsset, prefetchAssets, agentTint } from "@clappkit";
import { useSnapshot, type Snapshotish } from "@clappkit";
import { invoke } from "@tauri-apps/api/core";

/**
 * Open a form on the university's site, in the user's real browser.
 *
 * Not an `<a target="_blank">`: WebView2 raises `NewWindowRequested` for those and Tauri
 * swallows it, so the link silently does nothing. The core hands the URL to the OS.
 */
export async function openUrl(url: string): Promise<void> {
  try {
    await invoke("open_url", { url });
  } catch (e) {
    console.error("open_url failed", e);
  }
}

/**
 * A URL that renders the form IN the browser, rather than downloading it.
 *
 * Browsers display PDFs natively, so those link straight to the university. They cannot
 * display Word or Excel at all — and 527 of the 791 forms are exactly that — so those go
 * through Microsoft's public Office viewer, which means the form's (public) URL is sent to
 * Microsoft. That is a real trade and the reason the window offers a second, always-direct
 * download button beside this one: nothing here happens without the user choosing it.
 */
export function viewUrl(doc: Doc): string {
  if (doc.ext.toLowerCase() === "pdf") return doc.url;
  return `https://view.officeapps.live.com/op/view.aspx?src=${encodeURIComponent(doc.url)}`;
}

/** Whether the view button will hand this form to the Office viewer. */
export function usesViewer(doc: Doc): boolean {
  return doc.ext.toLowerCase() !== "pdf";
}

/** Why a form is in the results. `code` means the query named it — a fact, not a ranking. */
export type Why = "code" | "match";

export type Level = "lisans" | "lisansustu";

/** What the university's site said about a document, and when. */
export type Live = (
  | { status: "current" }
  | { status: "newer"; rev: number; url: string }
  | { status: "changed"; modified: string }
  | { status: "gone" }
  | { status: "unknown"; reason: string }
) & { checkedAt: string };

/** Something to say ABOUT the results, before any of them. */
export type Notice =
  | { kind: "scope"; scope: "calendar" | "meeting" | "announcement" }
  | { kind: "weak" };

export type SyncInfo = {
  checkedAt: string;
  outcome: "current" | "updated" | "failed";
  built: string | null;
  remoteBuilt: string | null;
  error: string | null;
};

export type Filter = { lang: string | null; level: Level | null; type: string | null };
/** A filter change: a field set to "" clears that dimension; absent fields keep theirs. */
export type FilterReq = { lang?: string; level?: string; type?: string };

export type Doc = {
  id: string;
  code: string | null;
  /** The revision the document prints about itself (its filename's, when it prints none). */
  rev: number;
  revName: number;
  revDate: string | null;
  pubDate: string | null;
  unit: string | null;
  collection: string | null;
  level: Level | null;
  /** The site check, once it has run. */
  live: Live | null;
  liveText: string | null;
  lang: "tr" | "en";
  title: string;
  name: string;
  ext: string;
  url: string;
  /** The text could not be extracted — it answers name queries and no others. */
  titleOnly: boolean;
  saved: boolean;
  score: number | null;
  why: Why | null;
  snippet: string | null;
  /** Where in this form the query was found, best first. */
  passages: string[];
};

export type Stage =
  | { stage: "missing" }
  | { stage: "loading" }
  | { stage: "downloading"; percent: number }
  | { stage: "ready" }
  | { stage: "failed"; reason: string };

/** One thing that happened, and who did it. `who` is an agent id, or null for the human. */
export type Activity = {
  seq: number;
  who: string | null;
  /** The actor's display name, resolved by the CORE against the roster — so the window and
   *  `gturag status` label the same event identically. */
  whoName: string | null;
  action: "search" | "open" | "save" | "unsave" | "sort" | "filter" | "sync";
  detail: string;
};

export type Agent = {
  id: string;
  name: string;
  backend: string | null;
  model: string | null;
  avatar: string | null;
};

export type Snapshot = Snapshotish & {
  /** The app's identity, from the manifest — the wordmark reads this, not a constant. */
  app: { name: string };
  query: string;
  /** One sentence describing what is on screen, built by the core so both surfaces say the
   *  same thing about the same state. */
  title: string;
  /** A search is in flight. */
  searching: boolean;
  /** Who ran the search on screen — agent id, or null for the human. */
  searchedBy: string | null;
  searchedByName: string | null;
  /** The terms the index matched on — words AND their stems. Highlighting uses these
   *  rather than the raw query, so `danışmanımı` correctly marks `Danışman` in a title. */
  terms: string[];
  sort: "relevance" | "code" | "title";
  filter: Filter;
  notice: Notice | null;
  /** How the query on screen was read. */
  language: "tr" | "en";
  results: Doc[];
  total: number;
  page: number;
  open: Doc | null;
  saved: Doc[];
  provision: {
    model: Stage;
    index: Stage;
    ready: boolean;
    summary: string;
  };
  corpus: {
    documents: number; chunks: number; built: string; source: string;
    /** Where `sync` fetches a newer index from — carried by the index itself. */
    updateUrl: string | null;
    collections: string[];
  } | null;
  /** The last `sync`, and whether one is running. */
  sync: SyncInfo | null;
  syncing: boolean;
  agents: Agent[];
  /** What both surfaces have been doing, oldest first. */
  activity: Activity[];
};

/** Every command the window can send. The core answers the same set over the CLI channel. */
export type Cmd =
  | { cmd: "state" }
  | { cmd: "search"; query: string }
  | { cmd: "open"; id: string }
  | { cmd: "save"; id: string }
  | { cmd: "unsave"; id: string }
  | { cmd: "sort"; by: string }
  | { cmd: "filter"; filter: FilterReq | "clear" }
  | { cmd: "sync" };

/** An agent id resolved to its current display name. Ids are the key and names are for
 *  humans, so a rename relabels history in place rather than orphaning it. */
export function nameOf(agents: Agent[], id: string | null): string {
  if (id === null) return "Siz";
  return agents.find((a) => a.id === id)?.name ?? id;
}

/** The stem length index.rs truncates to. A term at least this long may match a word by
 *  prefix; anything shorter must match the whole word, or `ad` would light up `adres`. */
export const STEM_LEN = 5;

/** Turkish-aware fold, matching `index::tokenize` exactly: `I`→`ı`, `İ`→`i`. JavaScript's
 *  `toLocaleLowerCase("tr")` does this; the default locale does not. */
export function fold(s: string): string {
  return s.toLocaleLowerCase("tr");
}

/** Does this word count as a match for one of the index's terms? */
/** The index version as a human reads it. `built` carries a full ISO timestamp so two
 *  rebuilds on one day are distinguishable to `sync`; a person only wants the day. */
export function builtOn(built: string): string {
  return built.split("T")[0];
}

/** An ISO date as a Turkish reader writes the day. */
export function dmy(iso: string): string {
  const d = iso.slice(0, 10).split("-");
  return d.length === 3 ? `${d[2]}.${d[1]}.${d[0]}` : iso;
}

/** An ISO timestamp in the reader's own time zone. */
export function when(iso: string): string {
  const t = new Date(iso);
  return Number.isNaN(t.getTime())
    ? iso
    : t.toLocaleString("tr-TR", { dateStyle: "short", timeStyle: "short" });
}

export function levelLabel(l: Level): string {
  return l === "lisansustu" ? "Lisansüstü" : "Lisans";
}

/** `https://www.gtu.edu.tr`, from the archive's provenance. */
export function siteOf(source: string | null | undefined): string | null {
  try {
    return source ? new URL(source).origin : null;
  } catch {
    return null;
  }
}

export function isMatch(word: string, terms: string[]): boolean {
  const w = fold(word);
  return terms.some((t) => w === t || (t.length >= STEM_LEN && w.startsWith(t)));
}

export const EMPTY: Snapshot = {
  app: { name: "" },
  ok: true,
  rev: -1,
  query: "",
  title: "",
  searching: false,
  searchedBy: null,
  searchedByName: null,
  terms: [],
  sort: "relevance",
  filter: { lang: null, level: null, type: null },
  notice: null,
  language: "tr",
  results: [],
  total: 0,
  page: 25,
  open: null,
  saved: [],
  provision: { model: { stage: "missing" }, index: { stage: "missing" }, ready: false, summary: "starting…" },
  corpus: null,
  sync: null,
  syncing: false,
  agents: [],
  activity: [],
};

/**
 * A snapshot can arrive with fields absent — an older core, or an error reply that only
 * carried `ok`. Normalising here rather than guarding at forty call sites is what keeps
 * the components readable, and it means a missing array is `[]` rather than a crash.
 */
function doc(d: Doc): Doc {
  return { ...d, passages: d.passages ?? [], live: d.live ?? null };
}

function normalize(raw: Snapshot): Snapshot {
  return {
    ...EMPTY,
    ...raw,
    results: (raw.results ?? []).map(doc),
    saved: (raw.saved ?? []).map(doc),
    open: raw.open ? doc(raw.open) : null,
    filter: raw.filter ?? EMPTY.filter,
    notice: raw.notice ?? null,
    corpus: raw.corpus ? { ...raw.corpus, collections: raw.corpus.collections ?? [] } : null,
    agents: raw.agents ?? [],
    activity: raw.activity ?? [],
    terms: raw.terms ?? [],
    provision: raw.provision ?? EMPTY.provision,
  };
}

export function useApp() {
  return useSnapshot<Snapshot, Cmd>(EMPTY, { normalize, initial: { cmd: "state" } });
}

/** Percent for a stage that has one, else null. */
export function percentOf(s: Stage): number | null {
  return s.stage === "downloading" ? s.percent : null;
}
