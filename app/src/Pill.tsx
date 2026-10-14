import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { EngineEvent, Mode } from "./types";

type Phase = "listening" | "transcribing" | "cleaning_up" | "filing" | "saved" | "not_pasted";

const LABELS: Record<Phase, string> = {
  listening: "Listening",
  transcribing: "Transcribing",
  cleaning_up: "Cleaning up",
  filing: "Filing note",
  saved: "Saved",
  not_pasted: "Couldn't paste. It's in your history.",
};

/** The floating pill. Rust shows and hides the window; this only picks the text. */
export default function Pill() {
  const [phase, setPhase] = useState<Phase>("listening");
  const [mode, setMode] = useState<Mode>("paste");
  const [category, setCategory] = useState<string | null>(null);
  const [handsFree, setHandsFree] = useState(false);

  useEffect(() => {
    const unlisten = listen<EngineEvent>("engine", ({ payload }) => {
      switch (payload.kind) {
        case "listening":
          setMode(payload.mode);
          setHandsFree(payload.hands_free);
          setPhase("listening");
          break;
        case "transcribing":
        case "cleaning_up":
          setMode(payload.mode);
          setPhase(payload.kind);
          break;
        case "filing":
          setPhase("filing");
          break;
        case "done":
          if (payload.mode === "note") {
            setCategory(payload.category);
            setPhase("saved");
          } else {
            // Reset for next time, unless there's a message to read.
            setPhase(payload.pasted ? "listening" : "not_pasted");
          }
          break;
        case "ignored":
          setPhase("listening");
          break;
      }
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  }, []);

  let label = LABELS[phase];
  if (phase === "saved") label = category ? `Saved to ${category}` : "Note saved";
  else if (phase === "listening") {
    // Hands-free keeps recording after the key is released, so say how to stop.
    const what = mode === "note" ? "Note" : "Listening";
    label = handsFree ? `${what} · tap to stop, Esc to cancel` : mode === "note" ? "Note · Listening" : "Listening";
  }

  return (
    <div className="pill" data-state={phase} data-mode={mode}>
      <span className="pill-dot" aria-hidden />
      <span>{label}</span>
    </div>
  );
}
