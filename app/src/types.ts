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

/** storage::Note */
export interface Note {
  id: number;
  created_at: number;
  updated_at: number;
  raw_text: string;
  text: string;
  source_app: string | null;
  category_id: number | null;
  category: string | null;
}

/** storage::Category */
export interface Category {
  id: number;
  name: string;
  note_count: number;
}

/** ask::Source: a note an answer may cite as [number]. */
export interface Source {
  number: number;
  note_id: number;
  created_at: number;
  category: string | null;
  excerpt: string;
}

export type AskEvent = { kind: "sources"; sources: Source[] } | { kind: "token"; text: string };

export interface Status {
  state: "loading" | "ready" | "error";
  detail: string;
}

export type Mode = "paste" | "note";

/** engine::Event, tagged by `kind`. */
export type EngineEvent =
  | { kind: "loading" }
  | { kind: "ready"; model: string; llm: string | null; microphone: string; notes: boolean }
  | { kind: "listening"; mode: Mode }
  | { kind: "transcribing"; mode: Mode }
  | { kind: "cleaning_up"; mode: Mode }
  | { kind: "filing" }
  | { kind: "ignored"; reason: string }
  | { kind: "warning"; message: string }
  | { kind: "notes_changed" }
  | {
      kind: "done";
      mode: Mode;
      raw_text: string;
      cleaned_text: string;
      source_app: string | null;
      pasted: boolean;
      history_id: number | null;
      note_id: number | null;
      category: string | null;
    };
