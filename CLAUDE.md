# whisperflow

A personal, fully local Wispr Flow–style voice dictation app for Windows.

## Decisions (already made; don't revisit unless something is blocked)

- **Platform:** Windows only, personal use. Everything free and local: no paid APIs, no cloud.
- **Hardware:** Intel i5-12500H (12 cores / 16 threads), 16 GB RAM, Intel Iris Xe (no NVIDIA). Models run on the CPU.
- **Stack:** Rust core library (this crate), later wrapped in a Tauri + React desktop app.
- **Speech-to-text:** whisper.cpp via the `whisper-rs` crate, English-only models (`base.en` / `small.en`). Default model is **base.en**: the step 1 benchmark on real dictation gave a speed factor of ~0.10 for base.en vs ~0.36 for small.en (8 threads), with near-identical accuracy. small.en stays available via `--model small.en`.
- **Cleanup LLM (step 3):** Ollama with a small local model (~1.5–3B, e.g. Qwen 2.5) to remove filler words, fix punctuation, and resolve self-corrections ("Tuesday, no wait, Wednesday" → "Wednesday").
  - Default cleanup model is **qwen2.5:1.5b** (~1.2 s per sentence vs ~2.3 s for qwen2.5:3b; neither was clearly more accurate on `bench-cleanup`). `src/cleanup.rs` checks every output (`plausible`): output words must come from the transcript (catches the model answering/obeying the text) and most non-filler words must survive (catches summarizing); on failure the raw transcript is pasted. Few-shot examples are written in Whisper's punctuated style, which matters.
- **Q&A chat (step 5):** a 7B model via Ollama is fine; speed matters less there. Uses **qwen2.5:7b** (`src/ask.rs`): ~6 s to first words and ~13 s total once loaded, ~35 s for the first question while it loads.
- **Categorizing notes** also uses **qwen2.5:7b** (`src/notes.rs`, ~3 s per note). qwen2.5:3b was tried and either over-reused existing categories (dentist → Work) or invented odd ones ("Finance Apps"). The prompt asks for a `Topic:` line before the `Category:` line.
- **Push-to-talk:** low-level keyboard hook (needs key-down and key-up). Right Alt is swallowed by the hook while notes are on (a lone Alt release would open app menus), so it can't act as AltGr then.
  - Hold **Right Ctrl** = dictate and paste into the focused app. Kept in short-term history only.
  - Hold **Right Alt** = dictate a note into the knowledge base (not pasted).
  - Any history item can be promoted to a note with one click.
- **Pasting:** save the clipboard, put the text on it, simulate Ctrl+V, restore the original clipboard. Implemented in `src/paste.rs`: saves every memory-backed clipboard format (text, HTML, files, images via CF_DIB), waits 300 ms after Ctrl+V before restoring, and marks both writes `ExcludeClipboardContentFromMonitorProcessing` so dictations stay out of Win+V history and cloud sync.
- **Storage:** SQLite. Each dictation stores raw text, cleaned text, timestamp, and source app.
- **Categories:** the LLM invents categories but must reuse existing ones when they fit (pass it the current list). "File this under X" at the start or end of a dictation is stripped and uses/creates category X. The UI must allow renaming and merging categories.
- **Notes Q&A:** RAG. Embeddings with `nomic-embed-text` via Ollama, vectors in SQLite via `sqlite-vec`, chat window inside the app, answers cite source notes with dates. Notes can be long and rambling (several minutes), so chunk before embedding.
- **Retention:** notes kept forever (editable/deletable). Paste history auto-pruned after 7 days. Audio deleted right after transcription (`--save-wav` debug flag is the only exception).
- **UI design:** minimal and restrained. Clean spacing, no decorative gradients/orbs/rings, one typographic treatment.

## Roadmap

1. ~~Hold key to record → transcribe locally → print text, plus a model benchmark~~ (done)
2. ~~Paste the text into whatever app is focused~~ (done)
3. ~~Ollama cleanup pass~~ (done)
4. ~~Tauri app: small floating "listening" pill, tray icon, history window, start-at-login option~~ (done)
5. ~~Notes key (Right Alt), auto-categorization, then the Q&A chat window~~ (done)
6. Hands-free mode: a quick lone tap of Right Ctrl / Right Alt starts recording and another tap stops it; holding still works as push-to-talk. Esc cancels, auto-stop after 10 minutes. ← **current step**

## Working agreements

- Don't start the next step until the user confirms the current one works.
- Commit to git after each working milestone.
- Keep the existing design and module structure; don't swap libraries unless something is truly broken, and explain why first.

## Build notes

- Needs Rust (MSVC toolchain), CMake, MSVC C++ build tools, and LLVM (for libclang). See README.md.
- whisper-rs bindings are generated at build time with bindgen; `.cargo/config.toml` sets `LIBCLANG_PATH` to `C:\Program Files\LLVM\bin`. The bundled bindings (`WHISPER_DONT_GENERATE_BINDINGS`) are Linux-generated and do not compile on Windows.
- `.cargo/config.toml` also sets `CMAKE_C_FLAGS_RELEASE`/`CMAKE_CXX_FLAGS_RELEASE` with `/O2`: the cmake crate otherwise drops optimization and whisper.cpp runs ~6x slower. After changing these, run `cargo clean --release -p whisper-rs-sys` (cargo does not detect the change).
- Ollama is installed in `D:\Ollama` with `OLLAMA_MODELS=D:\Ollama\models` (user env var). Drive C: is nearly full, so put large installs and downloads on D:.
- The desktop app is `app/` (Tauri 2 + React + Vite); `app/src-tauri` is a workspace member, so it shares `target/` with the core crate. Build with `npx tauri build --no-bundle` from `app/` (output `target\release\whisperflow-app.exe`). Use release builds: debug builds compile whisper.cpp without the `/O2` fix. Set `npm_config_cache=D:\npm-cache` when running npm (C: is short on space).
- `src/engine.rs` owns the dictation loop and emits `Event`s; the CLI prints them and the app forwards them to both windows as the `engine` event. Keep new pipeline behaviour in the engine, not in the app or CLI.
- The pill must never take focus (the paste goes to the focused window): it is `focusable(false)`, click-through, and shown with Win32 `SW_SHOWNOACTIVATE` in `app/src-tauri/src/pill.rs`. Don't switch it to `window.show()`.
- The keyboard hook ignores injected key events, so push-to-talk can't be tested with simulated keys; the user has to test it.
- Notes: `src/notes.rs` (file-under parsing, categorizing, chunking, indexing), `src/ask.rs` (RAG answer with [n] citations), `src/ollama.rs` (shared client). Storage tables: `notes`, `categories`, `note_chunks`, and the sqlite-vec table `chunk_vectors` (rowid = chunk id, cosine). Notes saved while Ollama is down are indexed later (`index_pending`, run at engine start and before each question).
- `WHISPERFLOW_DB` overrides the database path (app and CLI); use a scratch DB for testing, never the user's real one. CLI: `note`, `ask`, `reindex`, and `--notes` for Right Alt in push-to-talk.
- Tauri capabilities (`app/src-tauri/capabilities/default.json`) list window labels (`main`, `pill`). If a window is renamed, update it, or event listening silently fails while app commands still work.
- Memory is tight: 16 GB RAM, the 7B model takes ~5 GB, and the browser often uses 4+ GB. Out-of-memory / "paging file is too small" errors come from the commit limit, not bugs. Build with `CARGO_BUILD_JOBS=4`. The user was advised to move most of the page file to D:.
- On Windows `Main.tsx` and `main.tsx` are the same file; the window shell is `App.tsx`.
- Keep the project at a short path (it lives in `D:\whisperFlow`): MSVC `link.exe` fails with LNK1104 when build paths exceed 260 characters.
- Models live in `models\` (`scripts\download-models.ps1`); they're git-ignored.
