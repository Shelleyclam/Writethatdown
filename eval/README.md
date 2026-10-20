# Evals

`whisperflow eval` runs recordings of real dictation through the same pipeline the app
uses (Whisper, then cleanup), without pasting anything, and measures:

- **Raw WER**: how many words Whisper got wrong compared with what was actually said.
- **Final WER**: how far the text that would be pasted is from what was wanted.
- **Drift**: cases where cleanup added or dropped meaningful words (not fillers or
  punctuation) in a way the wanted text doesn't explain.
- **Latency**: median (p50) and 95th percentile (p95) for transcription, cleanup, and
  the total, on the CPU.

Every metric is also broken down by tag, so you can see, for example, how names or long
notes do compared with everything else.

## 1. Record

Quit the Writethatdown app first (it would paste what you say), then from the project
folder run:

```powershell
.\target\release\whisperflow.exe record
```

Dictate exactly as you normally would: hold Right Ctrl and talk, or tap it to start and
tap again to stop. Each recording is saved as the next number (`001.wav`, `002.wav`, …)
in `eval\cases\`. Press Ctrl+C when you're done. To record one clip with a name of your
choice, use `record 014-names`.

The transcript is deliberately not shown, so it can't influence what you write down as
"said".

Tips:

- **Don't read from a script.** Read-aloud speech has no "um"s or self-corrections, so
  cleanup would have nothing to do. Decide roughly what to say, then say it naturally.
- **A good mix of lengths** (about 30 clips) looks like real use, which is mostly short:

  | Length | Clips | What it tests |
  |---|---|---|
  | 1–3 s | ~5 | Very short clips, and the first word being cut off |
  | 3–10 s | ~12 | Everyday messages: a reply, a search, one sentence |
  | 10–30 s | ~8 | A paragraph, with fillers and self-corrections |
  | 30 s – 2 min | ~4 | Thinking out loud: does cleanup drop or summarize? |
  | 3–5 min | 1–2 | Long notes (tag these `note`) |

- **Spread the hard parts across lengths**: names and jargon from your own life,
  self-corrections ("Tuesday, no wait, Wednesday"), numbers and times, a few clips with
  background noise, and a couple spoken faster or quieter than usual.

## 2. Fill in the sheet

Open `eval\cases.csv` in Excel. One row per recording:

| Column | What goes in it |
|---|---|
| `file` | The recording's name, e.g. `001.wav` (`.wav` can be left out) |
| `said` | Exactly what you said, written down while listening back |
| `wanted` | The text you wanted pasted |
| `tags` | Free labels, separated by commas |

**`said`**: listen to the recording and type every word, including "um", "uh", repeated
words and false starts. Write numbers the way you said them ("three thirty"). Don't look
at what the app transcribed first: if you copy Whisper's output, you'll copy its mistakes
too and the score will be meaningless.

**`wanted`**: the text exactly as you'd want it to appear, with your punctuation,
capitalization and spelling ("3:30", "Shelley", "Pinecone").

**`tags`**: anything you want results broken down by, for example `names`, `jargon`,
`self-correction`, `filler`, `numbers`, `noisy`, `fast`, `accent:indian`, `code-mixed`.
Two tags are special:

- `note` runs the case through the note path (Right Alt): long text is cleaned in
  pieces, and "file this under …" is removed.
- Length tags (`len:short` under 10 s, `len:medium` up to 30 s, `len:long` above) are
  added automatically. Don't add them yourself.

Extra columns (your own comments, say) are ignored. Save with **File > Save As > CSV
UTF-8 (Comma delimited)** (plain CSV works too). Excel turns `001` into `1`; that's
fine, a plain number means the numbered recording. Rows with an empty `said` or
`wanted` are skipped with a message, so you can record first and fill in the sheet
later.

To check the sheet against the recordings without running any models:

```powershell
.\target\release\whisperflow.exe eval --check
```

## 3. Run

```powershell
.\target\release\whisperflow.exe eval                    # with cleanup (the app's default)
.\target\release\whisperflow.exe eval --compare          # with and without cleanup, side by side
.\target\release\whisperflow.exe eval --no-cleanup       # Whisper only
.\target\release\whisperflow.exe eval --model small.en   # another Whisper model
.\target\release\whisperflow.exe eval --tags names,note  # only cases with these tags
.\target\release\whisperflow.exe eval --judge            # also ask qwen2.5:7b about meaning
.\target\release\whisperflow.exe eval --show             # print said / heard / wanted / pasted per case
```

Runs limited with `--tags` don't update `eval\RESULTS.md`, so a quick look at a few cases
never replaces the full summary.

Cleanup needs Ollama running. If it isn't, the eval stops instead of quietly measuring
the raw text.

`--judge` gives a second opinion on drift: after the run, qwen2.5:7b reads what Whisper
heard, what you wanted, and what cleanup produced, and answers whether cleanup kept the
meaning, with a one-sentence reason. It adds a few seconds per case (about 30 seconds
more for the first, while the model loads). It's reported next to the word-based drift
check, not instead of it: a language model's verdicts vary with wording, and its reason
sometimes contradicts its own verdict. Where the two disagree, read the case yourself in
the per-case CSV.

## 4. Read the results

Each run writes:

- a table in the terminal;
- `eval\RESULTS.md`: the same tables as Markdown, ready to paste into the main README;
- `eval\results\<date>-<model>.json`: everything, per case and per tag (`--out` changes
  the file);
- `eval\results\<date>-<model>-<variant>.csv`: one row per case with what you said,
  what Whisper heard, what you wanted and what would have been pasted, side by side.
  Open it in Excel to go through the mistakes. In `drift_words`, `-word` means cleanup
  dropped a word you wanted and `+word` means it added one you didn't say or want.

How scoring works (see `src/eval/metrics.rs`):

- Words are compared after lowercasing and removing punctuation; curly apostrophes
  count as straight ones, and number words up to 99 count as digits ("three thirty"
  equals "3:30").
- WER for a group is total errors divided by total words, so a long recording counts
  for more than a three-word one.
- Drift ignores filler and function words ("um", "the", "so"), and changes that your
  `wanted` text explains: dropping "Tuesday" from "Tuesday, no wait, Wednesday" is
  expected. A case is flagged when 2 or more content words change, or more than 10% of
  them.

Known limits: numbers above 99 and words written with and without a hyphen ("e-mail"
vs "email") count as differences. With ~30 cases, p95 is close to the slowest case.

## Privacy

Recordings (`eval\cases\`) and results (`eval\results\`) are kept out of git: they're
your voice and your words. `eval\cases.csv` and `eval\RESULTS.md` are committed. If you'd
rather keep your sentences private too, add `eval/cases.csv` to `.gitignore`.
