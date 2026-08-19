// The window: a search bar, what it found, and WHERE in each form it was found.
//
// Not a second chat surface — the agent handles conversation, and Clatch carries it. There
// is deliberately no "ask the agent" button: a clapp's window is for the human's own
// actions, and a button that prompts on their behalf inverts that (PLAYBOOK, field notes).

import { useEffect, useMemo, useRef, useState } from "react";
import {
  useApp, useAsset, prefetchAssets, agentTint, isMatch, fold, nameOf,
  openUrl, viewUrl, usesViewer,
  percentOf, type Doc, type Agent, type Activity, type Snapshot,
} from "./bridge";

/** Debounced so a fast typist does not run a search per keystroke. */
const TYPING_SETTLE_MS = 220;

export default function App() {
  const { state, run } = useApp();
  const [draft, setDraft] = useState("");
  const settled = useRef<number | undefined>(undefined);
  // The agent can search too, and when it does the box must show what it searched for.
  // Tracked against the last query WE sent, so the human's own typing is never yanked
  // out from under them by the echo of their own command.
  const sent = useRef("");

  useEffect(() => {
    if (state.query !== sent.current) {
      sent.current = state.query;
      setDraft(state.query);
    }
  }, [state.query]);

  useEffect(() => {
    prefetchAssets(state.agents.map((a) => a.avatar));
  }, [state.agents]);

  function search(query: string) {
    sent.current = query;
    run({ cmd: "search", query });
  }

  function onType(value: string) {
    setDraft(value);
    window.clearTimeout(settled.current);
    settled.current = window.setTimeout(() => search(value), TYPING_SETTLE_MS);
  }

  return (
    <div className="app">
      <header className="header">
        <div className="brand">
          <Butterfly />
          <div className="brandtext">
            <span className="wordmark">GTÜ Formlar</span>
            {state.corpus && (
              <span className="corpus">
                {state.corpus.documents} form · {state.corpus.chunks} pasaj ·{" "}
                {state.corpus.built}
              </span>
            )}
          </div>
        </div>
        <div className="header-right">
          <SyncButton state={state} onSync={() => run({ cmd: "sync" })} />
          <div className="agents">
            {state.agents.map((a) => (
              <AgentChip key={a.id} agent={a} />
            ))}
          </div>
        </div>
      </header>

      <div className="stage">
        <div className="searchbar">
          <SearchIcon />
          <input
            autoFocus
            value={draft}
            placeholder="Ne yapmak istiyorsunuz? — “danışman değiştirmek”, “staj başvurusu”, FR-0083"
            onChange={(e) => onType(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                window.clearTimeout(settled.current);
                search(draft);
              }
              if (e.key === "Escape") onType("");
            }}
          />
          {draft && (
            <button className="clear" onClick={() => onType("")} title="Temizle">
              ×
            </button>
          )}
        </div>

        <Provisioning state={state} onRetry={() => run({ cmd: "sync" })} />

        <Strip state={state} />

        <div className="panes">
          <section className="results">
              <Toolbar state={state} onSort={(by) => run({ cmd: "sort", by })} />
              {state.results.length === 0 ? (
                <Empty state={state} />
              ) : (
                state.results.map((d) => (
                  <Row
                    key={d.id}
                    doc={d}
                    terms={state.terms}
                    active={state.open?.id === d.id}
                    onOpen={() => run({ cmd: "open", id: d.id })}
                    onToggleSave={() => run({ cmd: d.saved ? "unsave" : "save", id: d.id })}
                  />
                ))
              )}
            </section>

            <aside className="detail">
              <Detail
                doc={state.open}
                terms={state.terms}
                onToggleSave={(d) => run({ cmd: d.saved ? "unsave" : "save", id: d.id })}
              />
              <Saved
                docs={state.saved}
                onOpen={(id) => run({ cmd: "open", id })}
                onRemove={(id) => run({ cmd: "unsave", id })}
              />
              <Feed state={state} />
            </aside>
        </div>
      </div>
    </div>
  );
}

/**
 * Mark the words the INDEX matched, not the ones the query happens to spell.
 *
 * `terms` carries the tokenizer's output — words AND their 5-character stems — so a search
 * for `danışmanımı` lights up `Danışman` in a title, which a substring search of the raw
 * query never would. Splitting on non-letters (rather than regex-replacing the query) is
 * what keeps that honest: every word is judged by the same rule the ranker used.
 */
function Mark({ text, terms }: { text: string; terms: string[] }) {
  const parts = useMemo(() => {
    if (!terms.length || !text) return [{ s: text, hit: false }];
    const out: { s: string; hit: boolean }[] = [];
    let buf = "";
    let bufIsWord = false;
    const flush = () => {
      if (!buf) return;
      out.push({ s: buf, hit: bufIsWord && isMatch(buf, terms) });
      buf = "";
    };
    for (const ch of text) {
      const isWord = /[\p{L}\p{N}]/u.test(ch);
      if (isWord !== bufIsWord) {
        flush();
        bufIsWord = isWord;
      }
      buf += ch;
    }
    flush();
    // Merge neighbouring non-matches so React renders far fewer nodes.
    return out.reduce<{ s: string; hit: boolean }[]>((acc, p) => {
      const last = acc[acc.length - 1];
      if (last && !last.hit && !p.hit) last.s += p.s;
      else acc.push({ ...p });
      return acc;
    }, []);
  }, [text, terms]);

  return (
    <>
      {parts.map((p, i) => (p.hit ? <mark key={i}>{p.s}</mark> : <span key={i}>{p.s}</span>))}
    </>
  );
}

/** A passage, with its leading title line dropped — the title is already above it. */
function passageBody(passage: string): string {
  const nl = passage.indexOf("\n");
  return (nl >= 0 ? passage.slice(nl + 1) : passage).trim();
}

function Row({
  doc, terms, active, onOpen, onToggleSave,
}: {
  doc: Doc; terms: string[]; active: boolean; onOpen: () => void; onToggleSave: () => void;
}) {
  // Only passages that actually contain a matched word are worth showing as evidence;
  // a passage that matched semantically has nothing to point at, and claiming otherwise
  // by showing it under a "where it matched" heading would be a small lie.
  const evidence = doc.passages
    .map(passageBody)
    .filter((p) => p && p.split(/[^\p{L}\p{N}]+/u).some((w) => isMatch(w, terms)))
    .slice(0, 2);

  return (
    <article className={`row ${active ? "active" : ""}`} onClick={onOpen}>
      <div className="row-head">
        <span className="code">{doc.code ?? "—"}</span>
        <h3 className="title">
          <Mark text={doc.title} terms={terms} />
        </h3>
        {doc.why === "code" && <span className="badge exact">tam eşleşme</span>}
        <span className={`badge lang ${doc.lang}`}>{doc.lang.toUpperCase()}</span>
        <span className="badge ext">{doc.ext.toUpperCase()}</span>
        <button
          className={`save ${doc.saved ? "on" : ""}`}
          title={doc.saved ? "Listeden çıkar" : "Listeye ekle"}
          onClick={(e) => {
            e.stopPropagation();
            onToggleSave();
          }}
        >
          {doc.saved ? "★" : "☆"}
        </button>
      </div>

      {evidence.length > 0 ? (
        <div className="evidence">
          {evidence.map((p, i) => (
            <p key={i} className="passage-line">
              <Mark text={p} terms={terms} />
            </p>
          ))}
        </div>
      ) : (
        doc.snippet && <p className="snippet">{passageBody(doc.snippet).slice(0, 200)}</p>
      )}

      {doc.titleOnly && (
        // Honest rather than flattering: this document's body could not be read, so it
        // will only ever answer questions about its name.
        <p className="thin">Yalnızca başlıkla dizinlendi — içeriği okunamadı</p>
      )}
    </article>
  );
}

function Detail({
  doc, terms, onToggleSave,
}: {
  doc: Doc | null; terms: string[]; onToggleSave: (d: Doc) => void;
}) {
  if (!doc) {
    return (
      <div className="detail-empty">
        <Butterfly muted />
        <p>Bir form seçin.</p>
      </div>
    );
  }
  return (
    <div className="detail-card">
      <div className="detail-code">
        {doc.code ?? "—"} <span className="rev">R{doc.rev}</span>
      </div>
      <h2>{doc.title}</h2>
      <dl>
        <dt>Dosya</dt><dd>{doc.name}</dd>
        <dt>Tür</dt><dd>{doc.ext.toUpperCase()}</dd>
        <dt>Dil</dt><dd>{doc.lang === "tr" ? "Türkçe" : "İngilizce"}</dd>
      </dl>
      <div className="detail-actions">
        {/* Two links, because they are genuinely two different things. VIEW renders the
            form in the browser — which for Word and Excel means Microsoft's viewer, since
            no browser can display them. DOWNLOAD always goes straight to the university
            and touches nobody else. Both are <a> elements so they read and behave like
            links, but the navigation goes through the core: a bare target="_blank" is
            swallowed by the webview. */}
        <a
          className="primary"
          href={viewUrl(doc)}
          onClick={(e) => {
            e.preventDefault();
            void openUrl(viewUrl(doc));
          }}
        >
          Tarayıcıda görüntüle ↗
        </a>
        <a
          className="secondary"
          href={doc.url}
          onClick={(e) => {
            e.preventDefault();
            void openUrl(doc.url);
          }}
        >
          Dosyayı indir ({doc.ext.toUpperCase()})
        </a>
        <button onClick={() => onToggleSave(doc)}>
          {doc.saved ? "★ Listede" : "☆ Listeye ekle"}
        </button>
      </div>
      {usesViewer(doc) && (
        // Said out loud rather than buried: the viewer is a third party, and a user who
        // would rather not involve one has the download button right above.
        <p className="viewer-note">
          Word/Excel dosyaları tarayıcıda Microsoft görüntüleyicisi ile açılır.
        </p>
      )}
      {doc.passages.length > 0 && (
        <div className="detail-passages">
          <h4>Eşleşen bölümler</h4>
          {doc.passages.map((p, i) => (
            <pre key={i} className="passage">
              <Mark text={passageBody(p)} terms={terms} />
            </pre>
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * Update the form index, from the window.
 *
 * Lives in the HEADER because the header is the one thing on screen in every state. The
 * first version of this sat in the activity feed, which only renders once there are
 * results — so the one moment a user most wants it, opening the app to a blank search bar,
 * was the one moment it did not exist. A control for "before you start" cannot live behind
 * "after you have started".
 *
 * It reports on itself rather than firing silently: a button that looks identical before
 * and after being pressed reads as broken, and this one can take a while.
 */
function SyncButton({ state, onSync }: { state: Snapshot; onSync: () => void }) {
  const busy = state.provision.index.stage === "downloading";
  const failed = state.provision.index.stage === "failed";
  const percent = percentOf(state.provision.index);
  return (
    <button
      className={`syncbtn ${busy ? "busy" : ""} ${failed ? "failed" : ""}`}
      onClick={onSync}
      disabled={busy}
      title={
        state.corpus
          ? `Form dizinini güncelle — şu anki sürüm ${state.corpus.built}`
          : "Form dizinini indir"
      }
    >
      <span className="syncicon">⟳</span>
      {busy ? `Güncelleniyor… ${percent ?? 0}%` : "Dizini güncelle"}
    </button>
  );
}

function AgentChip({ agent }: { agent: Agent }) {
  const src = useAsset(agent.avatar);
  const initial = fold(agent.name || "?").trim().charAt(0).toLocaleUpperCase("tr");
  return (
    <div className="agent" title={`${agent.name}${agent.model ? ` · ${agent.model}` : ""}`}>
      {src ? (
        <img src={src} alt="" />
      ) : (
        <span className="monogram" style={{ background: agentTint(agent.id) }}>
          {initial}
        </span>
      )}
    </div>
  );
}

function Provisioning({ state, onRetry }: { state: Snapshot; onRetry: () => void }) {
  const { model, index, ready } = state.provision;
  if (ready) return null;

  const failed = model.stage === "failed" || index.stage === "failed";
  const percent = percentOf(index) ?? percentOf(model);
  const lexicalOnly = index.stage === "ready" && model.stage !== "ready";

  return (
    <div className={`provision ${failed ? "failed" : ""}`}>
      <div className="provision-text">
        <strong>{state.provision.summary}</strong>
        {lexicalOnly && (
          <span> — arama şimdiden çalışıyor. Model inince anlam araması da açılacak.</span>
        )}
      </div>
      {percent !== null && (
        <div className="bar">
          <div className="fill" style={{ width: `${percent}%` }} />
        </div>
      )}
      {failed && <button onClick={onRetry} className="retry">Tekrar dene</button>}
    </div>
  );
}

/**
 * The one line that is always on screen saying WHAT is being looked at and WHOSE it is.
 *
 * Taken from jlcpcb, which puts it exactly here for exactly this reason: the point of a
 * shared screen is knowing when it was not you. It also carries the in-flight state, so an
 * agent's search is visible WHILE it happens rather than only once it lands.
 */
function Strip({ state }: { state: Snapshot }) {
  return (
    <div className="strip">
      <span className="strip-title">{state.title || "GTÜ Formlar"}</span>
      {!state.query && !state.open && (
        <span className="strip-count">aramak için yazın</span>
      )}
      {state.searchedByName && (
        <span className="strip-by">arayan: {state.searchedByName}</span>
      )}
      {state.searching && <span className="strip-live">çalışıyor…</span>}
    </div>
  );
}

function Toolbar({ state, onSort }: { state: Snapshot; onSort: (by: string) => void }) {
  const options: [string, string][] = [
    ["relevance", "İlgi"],
    ["code", "Form no"],
    ["title", "Ad"],
  ];
  return (
    <div className="toolbar">
      <span className="count">{state.total > 0 ? `${state.total} sonuç` : ""}</span>
      <div className="sorts">
        {options.map(([value, label]) => (
          <button
            key={value}
            className={state.sort === value ? "on" : ""}
            onClick={() => onSort(value)}
            disabled={state.total === 0}
          >
            {label}
          </button>
        ))}
      </div>
    </div>
  );
}

function Empty({ state }: { state: Snapshot }) {
  if (!state.provision.ready && state.provision.index.stage !== "ready") {
    return <div className="empty"><p>Dizin henüz hazır değil.</p></div>;
  }
  // Never a different layout — just what this pane has to say. Before any search that is
  // an invitation; after one that found nothing it is a suggestion.
  if (!state.query) {
    return (
      <div className="empty">
        <Butterfly big />
        <p>Üniversitenin bütün formları, tek aramada.</p>
        <p className="hint">
          Formun adını bilmenize gerek yok — ne yapmak istediğinizi yazın. Numarasını
          biliyorsanız (<code>FR-0083</code>) doğrudan onu yazın. Ajanınız da buradan
          arayabilir; ikinizin bulduğu da bu ekranda görünür.
        </p>
      </div>
    );
  }
  return (
    <div className="empty">
      <p>“{state.query}” için sonuç yok.</p>
      <p className="hint">Başka kelimelerle deneyin, ya da form numarasıyla arayın.</p>
    </div>
  );
}

function Saved({
  docs, onOpen, onRemove,
}: {
  docs: Doc[]; onOpen: (id: string) => void; onRemove: (id: string) => void;
}) {
  if (docs.length === 0) return null;
  return (
    <div className="saved">
      <h3>Listem <span>{docs.length}</span></h3>
      {docs.map((d) => (
        <div key={d.id} className="saved-row">
          <button className="saved-open" onClick={() => onOpen(d.id)}>
            <span className="code">{d.code ?? "—"}</span> {d.title}
          </button>
          <button className="saved-rm" title="Çıkar" onClick={() => onRemove(d.id)}>×</button>
        </div>
      ))}
    </div>
  );
}

/**
 * What both surfaces have been doing, and who did it.
 *
 * This is the half of a clapp that a window usually forgets. The agent and the human act
 * on one state, so an agent's search already fills this screen — but without saying whose
 * it was, the human watches their own search box change for no visible reason. Every row
 * names its actor, and an agent's rows are tinted with the same colour as its avatar so
 * the two read as one identity.
 */
function Feed({ state }: { state: Snapshot }) {
  const rows = [...state.activity].reverse().slice(0, 10);
  return (
    <div className="feed">
      <h3>Etkinlik</h3>
      {rows.length === 0 ? (
        <p className="feed-empty">Henüz bir şey yapılmadı.</p>
      ) : (
        rows.map((a) => <FeedRow key={a.seq} item={a} agents={state.agents} />)
      )}
      {state.corpus && (
        <p className="feed-built">Dizin sürümü: {state.corpus.built}</p>
      )}
    </div>
  );
}

const VERBS: Record<Activity["action"], string> = {
  search: "aradı",
  open: "açtı",
  save: "listeye ekledi",
  unsave: "listeden çıkardı",
  sort: "sıraladı",
  sync: "dizini güncelledi",
};

function FeedRow({ item, agents }: { item: Activity; agents: Agent[] }) {
  const mine = item.who === null;
  return (
    <div className={`feed-row ${mine ? "mine" : "theirs"}`}>
      <span
        className="feed-who"
        // Keyed on the agent id, matching the avatar strip, so one agent is one colour.
        style={mine ? undefined : { color: agentTint(item.who!) }}
      >
        {item.whoName ?? nameOf(agents, item.who)}
      </span>
      <span className="feed-verb">{VERBS[item.action] ?? item.action}</span>
      <span className="feed-detail">{item.detail}</span>
    </div>
  );
}

/**
 * The mark. GTÜ's own emblem is a butterfly, so the app wears one — drawn here rather than
 * copied, because the university's crest is the university's, and this is an independent
 * tool. Two wings in the school's navy and orange, symmetric about a slim body.
 */
function Butterfly({ big, muted }: { big?: boolean; muted?: boolean }) {
  const size = big ? 72 : 26;
  return (
    <svg
      className={`butterfly ${muted ? "muted" : ""}`}
      width={size}
      height={size}
      viewBox="0 0 48 48"
      aria-hidden
    >
      <g className="wing-left">
        <path d="M23 24C17 10 9 6 5 10c-4 4-2 14 4 18-5 1-7 5-5 8 3 4 12 2 19-8z" />
      </g>
      <g className="wing-right">
        <path d="M25 24C31 10 39 6 43 10c4 4 2 14-4 18 5 1 7 5 5 8-3 4-12 2-19-8z" />
      </g>
      <g className="body">
        <path d="M24 13c1.2 0 2 1 2 2.4v17.2c0 1.4-.8 2.4-2 2.4s-2-1-2-2.4V15.4C22 14 22.8 13 24 13z" />
        <path d="M24 13c0-2-1.6-4-3.6-5" fill="none" strokeWidth="1.6" strokeLinecap="round" />
        <path d="M24 13c0-2 1.6-4 3.6-5" fill="none" strokeWidth="1.6" strokeLinecap="round" />
      </g>
    </svg>
  );
}

function SearchIcon() {
  return (
    <svg viewBox="0 0 24 24" width="20" height="20" fill="none" stroke="currentColor"
         strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="11" cy="11" r="8" />
      <path d="m21 21-4.3-4.3" />
    </svg>
  );
}
