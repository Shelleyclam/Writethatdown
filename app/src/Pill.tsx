import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import type { EngineEvent } from "./types";

type PillState = "listening" | "transcribing" | "cleaning_up" | "not_pasted";

const LABELS: Record<PillState, string> = {
  listening: "Listening",
  transcribing: "Transcribing",
  cleaning_up: "Cleaning up",
  not_pasted: "Couldn't paste. It's in your history.",
};

/** The floating pill. Rust shows and hides the window; this only picks the text. */
export default function Pill() {
  const [state, setState] = useState<PillState>("listening");

  useEffect(() => {
    const unlisten = listen<EngineEvent>("engine", ({ payload }) => {
      switch (payload.kind) {
        case "listening":
        case "transcribing":
        case "cleaning_up":
          setState(payload.kind);
          break;
        case "done":
          // Reset for next time, unless there's a message to read.
          setState(payload.pasted ? "listening" : "not_pasted");
          break;
        case "ignored":
          setState("listening");
          break;
      }
    });
    return () => {
      unlisten.then((stop) => stop());
    };
  }, []);

  return (
    <div className="pill" data-state={state}>
      <span className="pill-dot" aria-hidden />
      <span>{LABELS[state]}</span>
    </div>
  );
}
