// The one seam between the window and the core. Nothing else in src/ imports @clappkit,
// and nothing else calls `invoke` — so there is exactly one place where the shape of a
// snapshot is written down, and one place to look when it changes.

export { useAsset, prefetchAssets, agentTint } from "@clappkit";
import { useSnapshot, type Snapshotish } from "@clappkit";

/** Why a form is in the results. `code` means the query named it — a fact, not a ranking. */
export type Why = "code" | "match";

export type Doc = {
  id: string;
  code: string | null;
  rev: number;
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
};

export type Stage =
  | { stage: "missing" }
  | { stage: "downloading"; percent: number }
  | { stage: "ready" }
  | { stage: "failed"; reason: string };

export type Agent = {
  id: string;
  name: string;
  backend: string | null;
  model: string | null;
  avatar: string | null;
};

export type Snapshot = Snapshotish & {
  query: string;
  sort: "relevance" | "code" | "title";
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
  corpus: { documents: number; chunks: number; built: string; source: string } | null;
  agents: Agent[];
};

/** Every command the window can send. The core answers the same set over the CLI channel. */
export type Cmd =
  | { cmd: "state" }
  | { cmd: "search"; query: string }
  | { cmd: "open"; id: string }
  | { cmd: "save"; id: string }
  | { cmd: "unsave"; id: string }
  | { cmd: "sort"; by: string }
  | { cmd: "sync" };

export const EMPTY: Snapshot = {
  ok: true,
  rev: -1,
  query: "",
  sort: "relevance",
  results: [],
  total: 0,
  page: 25,
  open: null,
  saved: [],
  provision: { model: { stage: "missing" }, index: { stage: "missing" }, ready: false, summary: "starting…" },
  corpus: null,
  agents: [],
};

/**
 * A snapshot can arrive with fields absent — an older core, or an error reply that only
 * carried `ok`. Normalising here rather than guarding at forty call sites is what keeps
 * the components readable, and it means a missing array is `[]` rather than a crash.
 */
function normalize(raw: Snapshot): Snapshot {
  return {
    ...EMPTY,
    ...raw,
    results: raw.results ?? [],
    saved: raw.saved ?? [],
    agents: raw.agents ?? [],
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
