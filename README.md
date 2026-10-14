# Writethatdown

A fully local voice dictation app for Windows, in the spirit of Wispr Flow.
Speech recognition and every language model run on your own machine, on the CPU:
no cloud services, no API keys, nothing leaves your computer.

- **Dictate anywhere.** Hold **Right Ctrl**, speak, release: the text is transcribed
  with Whisper, cleaned up by a small local LLM (filler words, punctuation,
  "Tuesday, no wait, Wednesday" corrections), and pasted into whatever app has focus.
  Your clipboard is put back afterwards.
- **Hands-free.** Tap the key instead of holding it, talk as long as you like, tap again
  to stop. **Esc** cancels.
- **Voice notes.** Hold or tap **Right Alt** to dictate a note instead. Notes are filed
  into categories automatically, reusing existing ones when they fit. Say
  "file this under groceries" to choose one yourself.
- **Ask your notes.** Ask questions in plain language. Answers come only from your notes
  and cite the notes (with dates) they came from.
- **A quiet desktop app.** It lives in the tray, shows a small floating pill while you
  talk, and keeps a 7-day history of what you dictated. Notes are kept until you delete
  them.

Built with Rust ([whisper.cpp](https://github.com/ggml-org/whisper.cpp) via
`whisper-rs`), [Ollama](https://ollama.com), SQLite with
[sqlite-vec](https://github.com/asg017/sqlite-vec), and a Tauri 2 + React desktop app.

## Requirements

- Windows 10 or 11, 64-bit.
- 16 GB of RAM recommended. Dictation needs about 1.5 GB. The 7B model used for filing
  notes and answering questions needs about 5 GB while loaded (it unloads 2 minutes
  after use).
- About 9 GB of disk for the models.
- No GPU needed. On a 12th-gen Intel laptop CPU, a 10-second dictation takes about
  1 second to transcribe and 1 second to clean up.

## Setup

### 1. Build tools

In PowerShell:

```powershell
winget install Rustlang.Rustup
winget install Kitware.CMake
winget install LLVM.LLVM
winget install OpenJS.NodeJS.LTS
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Close and reopen PowerShell so the new tools are on your PATH.

LLVM is needed to generate the whisper.cpp bindings. If you install it somewhere other
than `C:\Program Files\LLVM`, update `LIBCLANG_PATH` in `.cargo/config.toml`.

### 2. Ollama and its models

```powershell
winget install Ollama.Ollama
```

Then, in a new PowerShell window:

```powershell
ollama pull qwen2.5:1.5b
ollama pull qwen2.5:7b
ollama pull nomic-embed-text
```

`qwen2.5:1.5b` cleans up dictation, `qwen2.5:7b` files notes and answers questions, and
`nomic-embed-text` makes notes searchable. To keep the models on another drive, set the
`OLLAMA_MODELS` environment variable before pulling them.

If Ollama isn't running, dictation still works and pastes the raw transcript.

### 3. Whisper models

From the project folder:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1
```

This downloads `base.en` (~148 MB) and `small.en` (~488 MB) into `models\`.

### 4. Build

```powershell
cargo build --release
cd app
npm install
npx tauri build --no-bundle
```

The first build compiles whisper.cpp and takes a few minutes. Keep the project at a
short path (like `C:\code\writethatdown`): the MSVC linker fails on paths longer than
260 characters.

## Using the app

Run `target\release\whisperflow-app.exe`. It sits in the tray.

- **Hold Right Ctrl** anywhere to dictate. The pill at the bottom of the screen shows
  Listening, then Transcribing, and disappears once the text is pasted.
- **Tap Right Ctrl** (a quick press on its own) for hands-free: it keeps recording until
  you tap it again. **Esc** cancels, and a forgotten recording stops after 10 minutes.
  Using Right Ctrl in a shortcut (like Ctrl+C) never starts a recording.
- **Hold or tap Right Alt** to dictate a note. The pill shows "Filing note", then where
  it was saved. While Writethatdown runs, Right Alt doesn't reach other apps, so it can't
  be used as AltGr.
- **History tab:** copy an entry again, see what Whisper heard before cleanup, save it as
  a note, or delete it. Entries older than 7 days are removed.
- **Notes tab:** browse by category, rename or merge categories, edit, recategorize or
  delete notes.
- **Ask tab:** ask about your notes and click a citation to open the note. The first
  question takes about 30 seconds while the model loads, then 10-15 seconds each.
- **Right-click the tray icon** for Start at login and Quit. Closing the window keeps the
  app running.

Everything is stored in `%APPDATA%\local.whisperflow\whisperflow.db`. Audio is never
saved. (Internally the project is still called `whisperflow`: the crates, the executables
and the data folder use that name.)

## Command-line tool

`target\release\whisperflow.exe` runs the same pipeline in a terminal. It's handy for
benchmarking and debugging.

```powershell
.\target\release\whisperflow.exe bench    # compare Whisper models on your voice
.\target\release\whisperflow.exe          # push-to-talk with Right Ctrl
```

| Option | What it does |
|---|---|
| `bench [clip.wav]` | Compare the downloaded Whisper models on one recording |
| `file <clip.wav>` | Transcribe (and clean up) a WAV file |
| `clean <text>` | Run the cleanup pass on typed text |
| `bench-cleanup [models]` | Compare cleanup models on built-in test sentences |
| `note <text>` / `ask <question>` | File a typed note / ask about your notes |
| `reindex` | Make notes saved while Ollama was off searchable |
| `--model <name>` | `base.en` or `small.en` (default: `base.en`) |
| `--threads <n>` | CPU threads for Whisper (default: up to 8) |
| `--llm <name>` | Ollama model for cleanup (default: `qwen2.5:1.5b`) |
| `--no-cleanup` | Use the raw Whisper transcript |
| `--notes` | Also use Right Alt for notes (saved in the app's database) |
| `--no-paste` | Only print the text, don't paste it |
| `--save-wav` | Keep each recording in `recordings\` for debugging |

Set `WHISPERFLOW_DB` to use a different database, for example a scratch one for testing.
Don't run the app and the command-line push-to-talk at the same time: both would paste.

## Troubleshooting

- **"only silence, ignored"**: Windows is using the wrong microphone. Pick the right one
  under Settings → System → Sound → Input, then restart Writethatdown.
- **Nothing is pasted into one particular app**: Windows blocks simulated keystrokes into
  apps running as administrator. Run Writethatdown as administrator too.
- **The first word gets cut off**: start speaking a beat after pressing the key. The
  microphone takes a moment to open.
- **Notes or questions fail with out-of-memory errors**: the 7B model needs about 5 GB of
  free memory. Close some browser tabs, or give Windows a larger page file.
- **Build fails mentioning cmake or a C++ compiler**: reopen PowerShell after installing
  the build tools, or build from "Developer PowerShell for VS 2022".

## Project layout

```
src/audio.rs       microphone capture, conversion to 16 kHz mono
src/hotkey.rs      Right Ctrl / Right Alt keys (Windows low-level keyboard hook)
src/engine.rs      the dictation loop: record, transcribe, clean up, paste or file, save
src/transcribe.rs  Whisper model loading and transcription
src/cleanup.rs     LLM cleanup pass, with checks that fall back to the raw text
src/paste.rs       paste into the focused app, saving and restoring the clipboard
src/storage.rs     SQLite: history, notes, categories, embedding vectors
src/notes.rs       filing notes: "file this under", categories, chunking, search index
src/ask.rs         answering questions from notes, with citations
src/ollama.rs      small client for the local Ollama server
src/main.rs        command-line tool
app/src-tauri/     desktop app backend: tray, pill window, commands
app/src/           desktop app UI (React)
scripts/           Whisper model downloader
```

## How it was built

The project grew in six steps:

1. Hold to record, transcribe locally, benchmark the Whisper models
2. Paste into whatever app is focused
3. Clean-up pass with a local LLM
4. Desktop app: floating pill, tray icon, history window
5. Notes key, automatic categories, and Q&A over notes
6. Hands-free mode: tap to start, tap to stop

`CLAUDE.md` has the design decisions and build notes behind each step.

## License

[MIT](LICENSE)
