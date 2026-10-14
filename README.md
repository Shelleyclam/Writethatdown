# whisperflow

A personal, fully local voice dictation app for Windows.

Hold **Right Ctrl**, speak, release, and the text is transcribed on your machine
with Whisper, cleaned up by a small local LLM (filler words, punctuation,
"Tuesday, no wait, Wednesday" corrections), and pasted into whatever app has focus.
Whatever you had on the clipboard is put back afterwards.

A desktop app lives in the tray, shows a small floating pill while you talk, and
keeps a 7-day history of what you dictated.

**Step 5 (this version):** hold **Right Alt** to dictate a note instead. Notes are
filed into categories automatically (or say "file this under X"), kept until you
delete them, and you can ask questions about them in the app's Ask tab. Answers
come only from your notes and cite them. The command-line tool is still there for
benchmarks and debugging.

## One-time setup

Open **PowerShell** and install the build tools:

```powershell
winget install Rustlang.Rustup
winget install Kitware.CMake
winget install LLVM.LLVM
winget install Microsoft.VisualStudio.2022.BuildTools --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Close and reopen PowerShell afterwards so the new tools are on your PATH, then check:

```powershell
cargo --version
cmake --version
```

## Ollama (for the cleanup pass)

Install Ollama on D: and keep its models there too (C: is short on space):

```powershell
winget install Ollama.Ollama --location D:\Ollama
[Environment]::SetEnvironmentVariable('OLLAMA_MODELS', 'D:\Ollama\models', 'User')
```

Then, in a new PowerShell window:

```powershell
ollama pull qwen2.5:1.5b
ollama pull qwen2.5:7b
ollama pull nomic-embed-text
```

`qwen2.5:1.5b` cleans up dictation, `qwen2.5:7b` files notes and answers questions
(about 5 GB of RAM while loaded), and `nomic-embed-text` makes notes searchable.

If Ollama isn't running, whisperflow says so at startup and pastes the raw transcript.

## Get the models

From the project folder:

```powershell
powershell -ExecutionPolicy Bypass -File scripts\download-models.ps1
```

This downloads `base.en` (~148 MB) and `small.en` (~488 MB) into `models\`.

## Build and run

```powershell
cargo build --release
```

The first build compiles whisper.cpp and takes a few minutes. Later builds are fast.

**1. Benchmark** (do this first):

```powershell
.\target\release\whisperflow.exe bench
```

Hold Right Ctrl, talk normally for 10–15 seconds, release. Both models transcribe
the same clip and you get a speed factor for each, plus a recommended default.

**2. Push-to-talk** (click into any text box first, then hold Right Ctrl):

```powershell
.\target\release\whisperflow.exe
.\target\release\whisperflow.exe --model base.en
```

Other options:

| Option | What it does |
|---|---|
| `--model <name>` | `base.en` or `small.en` (default: `base.en` if downloaded) |
| `--threads <n>` | CPU threads for Whisper (default 8). Worth trying 4, 6 and 12 in `bench`. |
| `--llm <name>` | Ollama model for cleanup (default `qwen2.5:1.5b`) |
| `--no-cleanup` | Paste the raw Whisper transcript, skip the LLM |
| `clean <text>` | Run the cleanup pass on typed text |
| `bench-cleanup [models]` | Compare cleanup models on built-in test sentences |
| `--notes` | Also hold Right Alt to dictate notes (saved in the app's database) |
| `note <text>` / `ask <question>` | File a typed note / ask about your notes |
| `reindex` | Make notes saved while Ollama was off searchable |
| `--no-paste` | Only print the text in the terminal, don't paste it |
| `--save-wav` | Keeps each recording in `recordings\` for debugging |
| `file <clip.wav>` | Transcribes an existing WAV file |

## Desktop app

Needs Node.js (`winget install OpenJS.NodeJS.LTS`). Build it from the `app` folder:

```powershell
cd app
npm install
npx tauri build --no-bundle
```

This produces `target\release\whisperflow-app.exe`. Start it, and it sits in the tray:

- **Hold Right Ctrl** anywhere to dictate. A pill at the bottom of the screen shows
  Listening, then Transcribing, and disappears once the text is pasted.
- **Or tap Right Ctrl** (a quick press on its own) for hands-free: it keeps recording
  after you let go, until you tap it again. This works for Right Alt notes too. **Esc**
  cancels a recording, and a forgotten hands-free recording stops after 10 minutes.
  Using Right Ctrl in a shortcut (like Ctrl+C) never starts a recording.
- **Click the tray icon** to open the history: copy an entry again, see what Whisper
  heard before cleanup (Original), or delete it. Entries older than 7 days are removed.
- **Hold Right Alt** to dictate a note. The pill shows "Filing note", then where it
  was saved. Say "file this under groceries" at the start or end to choose the category.
  While whisperflow runs, Right Alt doesn't reach other apps (so it can't be used as AltGr).
- **Notes tab:** browse by category, rename or merge categories, edit, recategorize or
  delete notes. **History** entries have "Save as note".
- **Ask tab:** ask about your notes. Click a citation to open the note. The first
  question after a while takes ~30 s while the model loads, then ~10-15 s each.
- **Right-click the tray icon** for Start at login and Quit. Closing the window keeps the
  app running.

History is stored in `%APPDATA%\local.whisperflow\whisperflow.db`.

Don't run the app and the command-line tool's push-to-talk at the same time: both
would paste. The app refuses to start twice.

## Troubleshooting

- **"only silence, ignored"**: Windows is using the wrong microphone. Set the right
  one under Settings → System → Sound → Input, then restart the program.
- **Build fails mentioning cmake or a C++ compiler**: reopen PowerShell after the
  installs above, or run the build from "Developer PowerShell for VS 2022".
- **Nothing gets pasted into one particular app**: if that app runs as administrator,
  Windows blocks pasting into it from a normal program. Run whisperflow as administrator too.
- **First word gets cut off**: start speaking a beat after pressing the key. The mic
  takes a moment to open; this gets fixed in a later step.

## Project layout

```
src/audio.rs       microphone capture, conversion to 16 kHz mono
src/hotkey.rs      Right Ctrl push-to-talk (Windows low-level keyboard hook)
src/paste.rs       paste into the focused app, saving and restoring the clipboard
src/cleanup.rs     LLM cleanup pass via Ollama, with checks that fall back to the raw text
src/transcribe.rs  Whisper model loading and transcription
src/storage.rs     SQLite: paste history (pruned after 7 days), notes, categories, vectors
src/notes.rs       filing notes: "file this under", categories, chunking, search index
src/ask.rs         answering questions from notes, with citations
src/ollama.rs      small client for the local Ollama server
src/engine.rs      the dictation loop: record, transcribe, clean up, paste, save
src/main.rs        command-line tool (run / bench / file / clean / bench-cleanup)
app/src-tauri/     desktop app: tray, pill window, history commands (Rust)
app/src/           desktop app UI: pill and history window (React)
scripts/           model downloader
```

## Roadmap

1. ~~Hold to record, transcribe locally, benchmark~~
2. ~~Paste the text into whatever app is focused~~
3. ~~Clean-up pass with a local LLM (Ollama): filler words, punctuation, self-corrections~~
4. ~~Tauri app: floating "listening" pill, tray icon, history window~~
5. ~~Notes key (Right Alt), auto-categories, and a chat window to ask questions about your notes~~
6. Hands-free mode: tap the key once to start, tap again to stop ← you are here
