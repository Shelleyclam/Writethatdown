// Mirrors of the Rust types sent over Tauri events and commands.

/** A row of paste history (storage::Entry). */
export interface Entry {
  id: number;
  /** Unix time in milliseconds. */
  created_at: number;
  raw_text: string;
  cleaned_text: string;
  source_app: string | null;
}

export interface Status {
  state: "loading" | "ready" | "error";
  detail: string;
}

/** engine::Event, tagged by `kind`. */
export type EngineEvent =
  | { kind: "loading" }
  | { kind: "ready"; model: string; llm: string | null; microphone: string }
  | { kind: "listening" }
  | { kind: "transcribing" }
  | { kind: "cleaning_up" }
  | { kind: "ignored"; reason: string }
  | { kind: "warning"; message: string }
  | {
      kind: "done";
      raw_text: string;
      cleaned_text: string;
      source_app: string | null;
      pasted: boolean;
      history_id: number | null;
    };
