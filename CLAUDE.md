# whisperflow

A personal, fully local Wispr Flow–style voice dictation app for Windows.

## Decisions (already made; don't revisit unless something is blocked)

- **Platform:** Windows only, personal use. Everything free and local: no paid APIs, no cloud.
- **Hardware:** Intel i5-12500H (12 cores / 16 threads), 16 GB RAM, Intel Iris Xe (no NVIDIA). Models run on the CPU.
- **Stack:** Rust core library (this crate), later wrapped in a Tauri + React desktop app.
- **Speech-to-text:** whisper.cpp via the `whisper-rs` crate, English-only models (`base.en` / `small.en`). The default model is chosen by the step 1 benchmark.
- **Cleanup LLM (step 3):** Ollama with a small local model (~1.5–3B, e.g. Qwen 2.5) to remove filler words, fix punctuation, and resolve self-corrections ("Tuesday, no wait, Wednesday" → "Wednesday").
- **Q&A chat (step 5):** a 7B model via Ollama is fine; speed matters less there.
- **Push-to-talk:** low-level keyboard hook (needs key-down and key-up).
  - Hold **Right Ctrl** = dictate and paste into the focused app. Kept in short-term history only.
  - Hold **Right Alt** = dictate a note into the knowledge base (not pasted).
  - Any history item can be promoted to a note with one click.
- **Pasting:** save the clipboard, put the text on it, simulate Ctrl+V, restore the original clipboard.
- **Storage:** SQLite. Each dictation stores raw text, cleaned text, timestamp, and source app.
- **Categories:** the LLM invents categories but must reuse existing ones when they fit (pass it the current list). "File this under X" at the start or end of a dictation is stripped and uses/creates category X. The UI must allow renaming and merging categories.
- **Notes Q&A:** RAG. Embeddings with `nomic-embed-text` via Ollama, vectors in SQLite via `sqlite-vec`, chat window inside the app, answers cite source notes with dates. Notes can be long and rambling (several minutes), so chunk before embedding.
- **Retention:** notes kept forever (editable/deletable). Paste history auto-pruned after 7 days. Audio deleted right after transcription (`--save-wav` debug flag is the only exception).
- **UI design:** minimal and restrained. Clean spacing, no decorative gradients/orbs/rings, one typographic treatment.

## Roadmap

1. Hold key to record → transcribe locally → print text, plus a model benchmark ← **current step**
2. Paste the text into whatever app is focused
3. Ollama cleanup pass
4. Tauri app: small floating "listening" pill, tray icon, history window, start-at-login option
5. Notes key (Right Alt), auto-categorization, then the Q&A chat window

## Working agreements

- Don't start the next step until the user confirms the current one works.
- Commit to git after each working milestone.
- Keep the existing design and module structure; don't swap libraries unless something is truly broken, and explain why first.

## Build notes

- Needs Rust (MSVC toolchain), CMake, and MSVC C++ build tools. See README.md.
- `.cargo/config.toml` sets `WHISPER_DONT_GENERATE_BINDINGS=1`, so LLVM/libclang is not required.
- Models live in `models\` (`scripts\download-models.ps1`); they're git-ignored.
