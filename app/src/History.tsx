import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { appName, errorText, formatTime, groupByDay } from "./format";
import type { Entry, Note } from "./types";

/** What was pasted with Right Ctrl over the last 7 days. */
export default function History({ onError }: { onError: (message: string) => void }) {
  const [entries, setEntries] = useState<Entry[]>([]);
  const [query, setQuery] = useState("");

  const load = useCallback(() => {
    invoke<Entry[]>("get_history")
      .then(setEntries)
      .catch((e) => onError(errorText(e)));
  }, [onError]);

  useEffect(() => {
    load();
    const stop = listen("history-changed", load);
    return () => {
      stop.then((s) => s());
    };
  }, [load]);

  const remove = (id: number) => {
    setEntries((rows) => rows.filter((r) => r.id !== id));
    invoke("delete_entry", { id }).catch((e) => {
      onError(errorText(e));
      load();
    });
  };

  const groups = useMemo(() => {
    const q = query.trim().toLowerCase();
    const shown = q
      ? entries.filter((e) => e.cleaned_text.toLowerCase().includes(q) || e.raw_text.toLowerCase().includes(q))
      : entries;
    return groupByDay(shown, (e) => e.created_at);
  }, [entries, query]);

  return (
    <main className="view">
      <input
        className="search"
        type="search"
        placeholder="Search history"
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
                <Item key={entry.id} entry={entry} onDelete={() => remove(entry.id)} onError={onError} />
              ))}
            </ul>
          </section>
        ))
      )}

      <footer className="footnote">History is kept for 7 days. Save anything you want to keep as a note.</footer>
    </main>
  );
}

function Item({ entry, onDelete, onError }: { entry: Entry; onDelete: () => void; onError: (m: string) => void }) {
  const [showRaw, setShowRaw] = useState(false);
  const [copied, setCopied] = useState(false);
  const [saving, setSaving] = useState<"idle" | "saving" | string>("idle");
  const changed = entry.raw_text !== entry.cleaned_text;
  const text = showRaw ? entry.raw_text : entry.cleaned_text;

  const copy = async () => {
    await navigator.clipboard.writeText(text);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  };

  const saveAsNote = async () => {
    setSaving("saving");
    try {
      const saved = await invoke<{ note: Note; warnings: string[] }>("promote_to_note", { id: entry.id });
      saved.warnings.forEach(onError);
      setSaving(saved.note.category ? `Saved to ${saved.note.category}` : "Saved as note");
    } catch (e) {
      onError(errorText(e));
      setSaving("idle");
    }
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
        <span className="item-actions" data-pinned={saving !== "idle" || undefined}>
          {saving === "idle" ? (
            <button onClick={saveAsNote}>Save as note</button>
          ) : (
            <span>{saving === "saving" ? "Saving…" : saving}</span>
          )}
          <button onClick={copy}>{copied ? "Copied" : "Copy"}</button>
          {changed && <button onClick={() => setShowRaw(!showRaw)}>{showRaw ? "Cleaned" : "Original"}</button>}
          <button onClick={onDelete}>Delete</button>
        </span>
      </div>
    </li>
  );
}
