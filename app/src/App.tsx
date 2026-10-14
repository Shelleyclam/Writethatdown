import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import Ask from "./Ask";
import History from "./History";
import Notes from "./Notes";
import type { EngineEvent, Status } from "./types";

type Tab = "history" | "notes" | "ask";

const TABS: [Tab, string][] = [
  ["history", "History"],
  ["notes", "Notes"],
  ["ask", "Ask"],
];

const WARNING_MS = 8000;

/** The main window: status line, tabs, and the three views. */
export default function App() {
  const [tab, setTab] = useState<Tab>("history");
  const [status, setStatus] = useState<Status>({ state: "loading", detail: "Loading models…" });
  const [warning, setWarning] = useState<string | null>(null);
  /** A note to scroll to when switching to Notes (from a Q&A citation). */
  const [focusNote, setFocusNote] = useState<number | null>(null);

  useEffect(() => {
    const stops = [
      listen<Status>("status", ({ payload }) => setStatus(payload)),
      listen<EngineEvent>("engine", ({ payload }) => {
        if (payload.kind === "warning") setWarning(payload.message);
      }),
    ];
    // Read the current status only once listening, so a change in between isn't missed.
    stops[0].then(() => invoke<Status>("get_status").then(setStatus));
    return () => {
      stops.forEach((p) => p.then((stop) => stop()));
    };
  }, []);

  useEffect(() => {
    if (!warning) return;
    const t = setTimeout(() => setWarning(null), WARNING_MS);
    return () => clearTimeout(t);
  }, [warning]);

  const openNote = (id: number) => {
    setFocusNote(id);
    setTab("notes");
  };

  return (
    <div className="app">
      <header className="app-header">
        <nav className="tabs" role="tablist">
          {TABS.map(([id, label]) => (
            <button key={id} role="tab" aria-selected={tab === id} className="tab" onClick={() => setTab(id)}>
              {label}
            </button>
          ))}
        </nav>
        <p className="status" data-state={status.state}>
          <span className="status-dot" aria-hidden />
          {status.detail}
        </p>
        {warning && <p className="notice">{warning}</p>}
      </header>

      {/* Views stay mounted so switching tabs keeps scroll position and the Ask conversation. */}
      <div hidden={tab !== "history"}>
        <History onError={setWarning} />
      </div>
      <div hidden={tab !== "notes"}>
        <Notes focusNote={focusNote} onFocused={() => setFocusNote(null)} onError={setWarning} />
      </div>
      <div hidden={tab !== "ask"} className="ask-tab">
        <Ask onOpenNote={openNote} />
      </div>
    </div>
  );
}
