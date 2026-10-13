# whisperflow

A personal, fully local voice dictation app for Windows.

**Step 2 (this version):** hold **Right Ctrl**, speak, release, and the text is
transcribed on your machine with Whisper and pasted into whatever app has focus.
Whatever you had on the clipboard is put back afterwards. There is also a
benchmark mode to pick the best model for your laptop.

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
| `--no-paste` | Only print the text in the terminal, don't paste it |
| `--save-wav` | Keeps each recording in `recordings\` for debugging |
| `file <clip.wav>` | Transcribes an existing WAV file |

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
src/transcribe.rs  Whisper model loading and transcription
src/main.rs        command-line tool (run / bench / file)
scripts/           model downloader
```

## Roadmap

1. ~~Hold to record, transcribe locally, benchmark~~
2. ~~Paste the text into whatever app is focused~~ ← you are here
3. Clean-up pass with a local LLM (Ollama): filler words, punctuation, self-corrections
4. Tauri app: floating "listening" pill, tray icon, history window
5. Notes key (Right Alt), auto-categories, and a chat window to ask questions about your notes
