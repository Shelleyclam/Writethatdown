import { Fragment, useEffect, useRef, useState } from "react";
import { Channel, invoke } from "@tauri-apps/api/core";
import { errorText, formatDate } from "./format";
import type { AskEvent, Source } from "./types";

interface Turn {
  question: string;
  answer: string;
  sources: Source[];
  state: "searching" | "answering" | "done" | "error";
  error?: string;
}

const EXCERPT_CHARS = 180;

/** Chat with your notes. Answers cite the notes they use as [1], [2]. */
export default function Ask({ onOpenNote }: { onOpenNote: (noteId: number) => void }) {
  const [turns, setTurns] = useState<Turn[]>([]);
  const [draft, setDraft] = useState("");
  const end = useRef<HTMLDivElement>(null);
  const busy = turns.some((t) => t.state === "searching" || t.state === "answering");

  useEffect(() => {
    end.current?.scrollIntoView({ block: "end" });
  }, [turns]);

  const update = (index: number, change: (t: Turn) => Partial<Turn>) =>
    setTurns((all) => all.map((t, i) => (i === index ? { ...t, ...change(t) } : t)));

  const send = async () => {
    const question = draft.trim();
    if (!question || busy) return;
    setDraft("");
    const history = turns.filter((t) => t.state === "done").map(({ question, answer }) => ({ question, answer }));
    const index = turns.length;
    setTurns((all) => [...all, { question, answer: "", sources: [], state: "searching" }]);

    const onEvent = new Channel<AskEvent>();
    onEvent.onmessage = (event) => {
      if (event.kind === "sources") update(index, () => ({ sources: event.sources, state: "answering" }));
      else update(index, (t) => ({ answer: t.answer + event.text }));
    };
    try {
      const answer = await invoke<string>("ask", { question, history, onEvent });
      update(index, () => ({ answer, state: "done" }));
    } catch (e) {
      update(index, () => ({ state: "error", error: errorText(e) }));
    }
  };

  return (
    <main className="view ask">
      <div className="conversation">
        {turns.length === 0 && (
          <p className="empty">
            Ask anything about your notes. Answers come only from your notes and cite the ones they use.
          </p>
        )}
        {turns.map((turn, i) => (
          <article key={i} className="turn">
            <p className="question">{turn.question}</p>
            <div className="answer">
              {turn.state === "searching" && <p className="muted">Searching your notes…</p>}
              {turn.state === "answering" && !turn.answer && (
                <p className="muted">
                  Reading {turn.sources.length} {turn.sources.length === 1 ? "note" : "notes"}…
                </p>
              )}
              {turn.answer && <AnswerText text={turn.answer} sources={turn.sources} onOpenNote={onOpenNote} />}
              {turn.state === "error" && <p className="notice-error">{turn.error}</p>}
            </div>
            {turn.sources.length > 0 && turn.state !== "searching" && (
              <ol className="sources">
                {turn.sources.map((s) => (
                  <li key={s.number}>
                    <button onClick={() => onOpenNote(s.note_id)}>
                      <span className="source-head">
                        [{s.number}] {formatDate(s.created_at)}
                        {s.category && ` · ${s.category}`}
                      </span>
                      <span className="source-excerpt">{shorten(s.excerpt)}</span>
                    </button>
                  </li>
                ))}
              </ol>
            )}
          </article>
        ))}
        <div ref={end} />
      </div>

      <form
        className="ask-form"
        onSubmit={(e) => {
          e.preventDefault();
          send();
        }}
      >
        <textarea
          rows={2}
          placeholder="Ask about your notes"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              send();
            }
          }}
        />
        <div className="ask-actions">
          {turns.length > 0 && (
            <button type="button" disabled={busy} onClick={() => setTurns([])}>
              New conversation
            </button>
          )}
          <button type="submit" className="primary" disabled={busy || !draft.trim()}>
            {busy ? "Answering…" : "Ask"}
          </button>
        </div>
      </form>
    </main>
  );
}

/** Render the answer with [1] / [1, 3] citations as links to the notes. */
function AnswerText({
  text,
  sources,
  onOpenNote,
}: {
  text: string;
  sources: Source[];
  onOpenNote: (noteId: number) => void;
}) {
  const parts = text.split(/(\[\d+(?:\s*,\s*\d+)*\])/g);
  return (
    <p className="answer-text">
      {parts.map((part, i) => {
        const match = part.match(/^\[([\d,\s]+)\]$/);
        if (!match) return <Fragment key={i}>{part}</Fragment>;
        const numbers = match[1].split(",").map((n) => Number(n.trim()));
        return (
          <span key={i} className="cites">
            {numbers.map((n) => {
              const source = sources.find((s) => s.number === n);
              return source ? (
                <button key={n} className="cite" title={formatDate(source.created_at)} onClick={() => onOpenNote(source.note_id)}>
                  {n}
                </button>
              ) : (
                <span key={n} className="cite">
                  {n}
                </span>
              );
            })}
          </span>
        );
      })}
    </p>
  );
}

function shorten(text: string): string {
  return text.length > EXCERPT_CHARS ? `${text.slice(0, EXCERPT_CHARS).trimEnd()}…` : text;
}
