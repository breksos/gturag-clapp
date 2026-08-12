// The window: a search bar, what it found, and the form you are looking at.
//
// Not a second chat surface — the agent handles conversation, and Clatch carries it. There
// is deliberately no "ask the agent" button: a clapp's window is for the human's own
// actions, and a button that prompts on their behalf inverts that (PLAYBOOK, field notes).

import { useEffect, useRef, useState } from "react";
import {
  useApp, useAsset, prefetchAssets, agentTint,
  percentOf, type Doc, type Agent, type Snapshot,
} from "./bridge";

/** Debounced so a dragging hand — or a fast typist — does not run 40 searches. */
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
      <Header state={state} />

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
            onToggleSave={(d) => run({ cmd: d.saved ? "unsave" : "save", id: d.id })}
          />
          <Saved
            docs={state.saved}
            onOpen={(id) => run({ cmd: "open", id })}
            onRemove={(id) => run({ cmd: "unsave", id })}
          />
        </aside>
      </div>
    </div>
  );
}

function Header({ state }: { state: Snapshot }) {
  return (
    <header className="header">
      <div className="brand">
        <span className="wordmark">GTÜ Formlar</span>
        {state.corpus && (
          <span className="corpus">
            {state.corpus.documents} form · güncellendi {state.corpus.built}
          </span>
        )}
      </div>
      <div className="agents">
        {state.agents.map((a) => (
          <AgentChip key={a.id} agent={a} />
        ))}
      </div>
    </header>
  );
}

/** An agent's avatar, or a monogram tinted from its id. Keyed on id — a rename is the same
 *  agent re-labelled, so the chip updates in place rather than being dropped and re-made. */
function AgentChip({ agent }: { agent: Agent }) {
  const src = useAsset(agent.avatar);
  const initial = (agent.name || "?").trim().charAt(0).toLocaleUpperCase("tr");
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

/** The one thing the human genuinely waits on, so it gets to be visible and specific. */
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
          <span>
            {" "}
            — arama şimdiden çalışıyor, kelime eşleşmesiyle. Model inince anlam araması da açılacak.
          </span>
        )}
      </div>
      {percent !== null && (
        <div className="bar">
          <div className="fill" style={{ width: `${percent}%` }} />
        </div>
      )}
      {failed && (
        <button onClick={onRetry} className="retry">
          Tekrar dene
        </button>
      )}
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
      <span className="count">
        {state.total > 0 ? `${state.total} sonuç` : ""}
      </span>
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
  if (!state.query) {
    return (
      <div className="empty">
        <p>Aramak için yukarıya yazın.</p>
        <p className="hint">
          Formun adını bilmenize gerek yok — ne yapmak istediğinizi anlatın. Form numarasını
          biliyorsanız (<code>FR-0083</code>) doğrudan onu yazın.
        </p>
      </div>
    );
  }
  if (!state.provision.ready && state.provision.index.stage !== "ready") {
    return <div className="empty"><p>Dizin henüz hazır değil.</p></div>;
  }
  return (
    <div className="empty">
      <p>“{state.query}” için sonuç yok.</p>
      <p className="hint">Başka kelimelerle deneyin, ya da form numarasıyla arayın.</p>
    </div>
  );
}

function Row({
  doc, active, onOpen, onToggleSave,
}: {
  doc: Doc; active: boolean; onOpen: () => void; onToggleSave: () => void;
}) {
  return (
    <div className={`row ${active ? "active" : ""}`} onClick={onOpen}>
      <div className="row-head">
        <span className="code">{doc.code ?? "—"}</span>
        <span className="title">{doc.title}</span>
        {doc.why === "code" && <span className="badge exact">tam eşleşme</span>}
        <span className={`badge lang ${doc.lang}`}>{doc.lang.toUpperCase()}</span>
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
      {doc.snippet && <p className="snippet">{doc.snippet.split("\n").slice(1).join(" ").slice(0, 220)}</p>}
      {doc.titleOnly && (
        // Honest rather than flattering: this document's body could not be read, so it
        // will only ever answer questions about its name.
        <p className="thin">Yalnızca başlıkla dizinlendi — içeriği okunamadı</p>
      )}
    </div>
  );
}

function Detail({ doc, onToggleSave }: { doc: Doc | null; onToggleSave: (d: Doc) => void }) {
  if (!doc) {
    return (
      <div className="detail-empty">
        <p>Bir form seçin.</p>
      </div>
    );
  }
  return (
    <div className="detail-card">
      <div className="detail-code">{doc.code ?? "—"} · R{doc.rev}</div>
      <h2>{doc.title}</h2>
      <dl>
        <dt>Dosya</dt><dd>{doc.name}</dd>
        <dt>Tür</dt><dd>{doc.ext.toUpperCase()}</dd>
        <dt>Dil</dt><dd>{doc.lang === "tr" ? "Türkçe" : "İngilizce"}</dd>
      </dl>
      <div className="detail-actions">
        {/* A real link to the university's own copy: the app is a finder, and the
            authoritative document is always theirs. */}
        <a className="primary" href={doc.url} target="_blank" rel="noreferrer">
          Formu aç
        </a>
        <button onClick={() => onToggleSave(doc)}>
          {doc.saved ? "★ Listede" : "☆ Listeye ekle"}
        </button>
      </div>
      {doc.snippet && <pre className="passage">{doc.snippet}</pre>}
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
          <button className="saved-rm" title="Çıkar" onClick={() => onRemove(d.id)}>
            ×
          </button>
        </div>
      ))}
    </div>
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
