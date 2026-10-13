import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { EngineEvent, Entry, Status } from "./types";

const WARNING_MS = 8000;

export default function History() {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [status, setStatus] = useState<Status>({ state: "loading", detail: "Loading models…" });
  const [warning, setWarning] = useState<string | null>(null);
  const [query, setQuery] = useState("");
  const [error, setError] = useState<string | null>(null);

  const load = useCallback(() => {
    invoke<Entry[]>("get_history")
      .then((rows) => {
        setEntries(rows);
        setError(null);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    load();
    invoke<Status>("get_status").then(setStatus);
    const stops = [
      listen("history-changed", load),
      listen<Status>("status", ({ payload }) => setStatus(payload)),
      listen<EngineEvent>("engine", ({ payload }) => {
        if (payload.kind === "warning") setWarning(payload.message);
      }),
    ];
    return () => {
      stops.forEach((p) => p.then((stop) => stop()));
    };
  }, [load]);

  useEffect(() => {
    if (!warning) return;
    const t = setTimeout(() => setWarning(null), WARNING_MS);
    return () => clearTimeout(t);
  }, [warning]);

  const remove = (id: number) => {
    setEntries((rows) => rows.filter((r) => r.id !== id));
    invoke("delete_entry", { id }).catch((e) => {
      setError(String(e));
      load();
    });
  };

  const groups = useMemo(() => groupByDay(filter(entries, query)), [entries, query]);

  return (
    <main className="history">
      <header className="history-header">
        <h1>History</h1>
        <p className="status" data-state={status.state}>
          <span className="status-dot" aria-hidden />
          {status.detail}
        </p>
        {warning && <p className="notice">{warning}</p>}
        {error && <p className="notice notice-error">{error}</p>}
      </header>

      <input
        className="search"
        type="search"
        placeholder="Search"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
      />

      {entries.length === 0 ? (
        <p className="empty">Nothing yet. Hold Right Ctrl, speak, and release.</p>
      ) : groups.length === 0 ? (
        <p className="empty">No matches.</p>
      ) : (
        groups.map(([day, rows]) => (
          <section key={day} className="day">
            <h2>{day}</h2>
            <ul>
              {rows.map((entry) => (
                <Item key={entry.id} entry={entry} onDelete={() => remove(entry.id)} />
              ))}
            </ul>
          </section>
        ))
      )}

      <footer className="footnote">Kept for 7 days.</footer>
    </main>
  );
}

function Item({ entry, onDelete }: { entry: Entry; onDelete: () => void }) {
  const [showRaw, setShowRaw] = useState(false);
  const [copied, setCopied] = useState(false);
  const changed = entry.raw_text !== entry.cleaned_text;
  const text = showRaw ? entry.raw_text : entry.cleaned_text;

  const copy = async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  return (
    <li className="item">
      <p className="item-text" data-raw={showRaw || undefined}>
        {text}
      </p>
      <div className="item-meta">
        <span>
          {formatTime(entry.created_at)}
          {entry.source_app && ` · ${appName(entry.source_app)}`}
        </span>
        <span className="item-actions">
          <button onClick={copy}>{copied ? "Copied" : "Copy"}</button>
          {changed && <button onClick={() => setShowRaw(!showRaw)}>{showRaw ? "Cleaned" : "Original"}</button>}
          <button onClick={onDelete}>Delete</button>
        </span>
      </div>
    </li>
  );
}

function filter(entries: Entry[], query: string): Entry[] {
  const q = query.trim().toLowerCase();
  if (!q) return entries;
  return entries.filter(
    (e) => e.cleaned_text.toLowerCase().includes(q) || e.raw_text.toLowerCase().includes(q),
  );
}

/** Entries are newest first; keep that order within and across days. */
function groupByDay(entries: Entry[]): [string, Entry[]][] {
  const groups = new Map<string, Entry[]>();
  for (const e of entries) {
    const day = dayLabel(e.created_at);
    groups.set(day, [...(groups.get(day) ?? []), e]);
  }
  return [...groups];
}

function dayLabel(ms: number): string {
  const date = new Date(ms);
  const today = new Date();
  const yesterday = new Date();
  yesterday.setDate(today.getDate() - 1);
  if (date.toDateString() === today.toDateString()) return "Today";
  if (date.toDateString() === yesterday.toDateString()) return "Yesterday";
  return date.toLocaleDateString(undefined, { weekday: "long", day: "numeric", month: "short" });
}

function formatTime(ms: number): string {
  return new Date(ms).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/** "chrome.exe" -> "chrome" */
function appName(exe: string): string {
  return exe.replace(/\.exe$/i, "");
}
