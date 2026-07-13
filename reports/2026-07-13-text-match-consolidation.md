# Greeting Check Consolidation: Transcript Matching Only (2026-07-13)

The Wav2Vec2 embedding pipeline has been removed. The health check is now a
single signal: the SpeechAnalyzer transcript of the captured call audio is
compared against an expected greeting text configured in `.env`.

## New Architecture

```
SIP call → RTP capture (G.711 @ 8 kHz) → decode + resample to 16 kHz
        → SpeechAnalyzer helper (WAV in → JSON transcript out)
        → token-level similarity vs EXPECTED_GREETING (src/greeting.rs)
        → alert via Pushover if below GREETING_MATCH_THRESHOLD,
          with expected text, heard transcript, and score in the message
```

- `EXPECTED_GREETING` (default `"thank you for calling cubic machinery"`) —
  the expected greeting text, stored verbatim; normalization is the matcher's
  job.
- `GREETING_MATCH_THRESHOLD` (default `0.75`) — similarity threshold,
  validated to be in (0.0, 1.0].
- `SpeechRecognizer` no longer needs a `Mutex` (no mutable model state); it is
  shared as a plain `Arc`.
- Alerts now include expected vs. heard text plus the score, so a failing
  check is diagnosable from the notification alone.

## Matcher Design (src/greeting.rs)

Both texts are normalized — lowercase, all non-alphanumeric characters
stripped, whitespace collapsed — and compared as **word sequences** with
token-level Levenshtein distance, so one misheard word costs one edit
regardless of its length. Two alignments are scored and the better one wins:

1. **Expected-inside-transcript** (fuzzy substring, free skip of transcript
   tokens before/after the window): handles captures longer than the expected
   text (greeting continues, leading noise).
2. **Transcript-as-prefix** (truncated capture): the transcript may match a
   leading prefix of the expected text, but only prefixes covering at least
   half of the expected tokens count.

Partial-capture semantics: a clean capture of at least ~half the expected
greeting passes; a two-word fragment never does. If real captures are
routinely shorter than `EXPECTED_GREETING`, shorten the greeting or raise
`LISTEN_DURATION_SECS` rather than lowering the threshold.

Implementation is two small dynamic-programming loops (~60 lines), no fuzzy-
matching dependencies. Char-level matching and embedding similarity were both
rejected: char-level over-weights long words; embeddings needed a 379 MB
model, ONNX Runtime, and self-updating reference state for a job the
transcript already answers.

## Threshold Rationale and Calibration

No real call captures exist in the repo (only the fixture `test_audio.wav`),
so calibration is the real fixture plus synthetic degradations of its
transcript. **The 0.75 default should be reviewed after the first few real
calls** — the alert message now carries the score, so drift is visible.

Real audio, end-to-end through the SpeechAnalyzer helper
(`tests/transcription.rs::fixture_transcript_matches_expected_greeting`):

| Input | Expected text | Similarity |
|---|---|---|
| test_audio.wav → "Thank you for calling cubic machinery. If you know your parties." | .env greeting (11 tokens) | **0.909** ✅ |
| same | config default ("thank you for calling cubic machinery") | **1.000** ✅ |

Synthetic degradations vs. the .env greeting, threshold 0.75
(`src/greeting.rs::calibration_synthetic_degradations`):

| Case | Score | Pass |
|---|---|---|
| Fixture transcript (real ASR output, "parties" for "party's") | 0.909 | ✅ |
| Fixture minus last word | 1.000 | ✅ |
| One word dropped mid-sentence | 0.818 | ✅ |
| Two misheard words | 0.818 | ✅ |
| First half of greeting only (clean truncation) | 0.833 | ✅ |
| Two words dropped + one misheard (3 edits in 11) | 0.727 | ❌ alerts |
| First third of greeting only | 0.500 | ❌ alerts |
| Dead-air ASR noise ("you you the") | 0.222 | ❌ alerts |
| Carrier intercept ("number ... not in service") | 0.111 | ❌ alerts |
| Empty transcript | 0.000 | ❌ alerts |

So 0.75 tolerates ~2 word-level errors in an 11-word greeting and rejects
everything that is not the greeting. The margin between the worst passing
case (0.818) and the threshold is comfortable; the borderline 3-edit case
correctly alerts.

## Expected Greeting Text

`.env` now sets:

```
EXPECTED_GREETING="Thank you for calling Cubic Machinery. If you know your party's"
```

This is the portion supported by repo evidence: the fixture audio is
truncated mid-word after "party's" (Whisper heard "...party's expected.",
SpeechAnalyzer hears "...your parties."). The full PBX greeting is not
recorded anywhere in the repo. **Owner action: after the next successful
check (or a manual `--once --save-audio` run), extend EXPECTED_GREETING with
the rest of the real greeting as transcribed.** Longer expected text makes
the check strictly more discriminating.

## What Was Removed

Direct dependencies: **ort** (ONNX Runtime), **ndarray**, **once_cell**
(only used by the ModelManager singleton), **insta** (dev; only embedding
snapshots). Cargo.lock shrank by **35 packages**.

Files deleted:
- `src/embedding.rs` (576 lines), `src/model_manager.rs` (170),
  `src/bin/test_embedding.rs` (110, embedding-only), all 8
  `src/snapshots/phonecheck__embedding__*.snap` (1,183 lines),
  `scripts/export_wav2vec2.py` (160)
- Reference-embedding self-update logic (similarity > 0.95 rewrite of
  `models/reference_embedding.bin`) — gone with `speech.rs`'s embedding path
- On disk: `models/wav2vec2_encoder.onnx` (1.5 MB), `.onnx.data` (378 MB),
  `reference_embedding.bin` (3 KB) — `models/` was then empty and removed;
  `.gitignore` and docs references dropped
- `EXPECTED_PHRASE` replaced by `EXPECTED_GREETING` in config, `.env`,
  `.env.example` (no other embedding-related env vars existed)

Kept: `test_audio.wav` (transcription fixture), SpeechAnalyzer helper
(unchanged), all SIP/RTP/STUN/scheduler/health code (unchanged).

LOC delta for the commit: **+540 / −1,823** across 27 files.

## openssl in Cargo.lock

Before: `ort`'s binary-download path pulled `ureq → native-tls →
openssl/openssl-sys/openssl-probe` into `Cargo.lock` (not compiled on this
target — `cargo tree -i openssl` already printed nothing — but present in the
lockfile). After removing ort: **openssl, openssl-sys, openssl-probe,
native-tls, and ureq are all gone from Cargo.lock**; `cargo tree -i openssl`
reports the package does not exist in the graph. Zero openssl remnants.

## Binary Size

| | Bytes | |
|---|---|---|
| Before | 28,387,136 | 27.1 MiB |
| After | 5,147,680 | 4.9 MiB |
| Delta | −23,239,456 | **−82%** |

Plus ~379 MB (361 MiB on disk) reclaimed from `models/`.

## Test and Gate Status

- `cargo build --release` — clean (4 pre-existing warnings in
  `rtp/receiver.rs`, present before this change; untouched)
- `cargo test --release` — **399 passed, 0 failed** (247 lib unit tests
  including 13 new matcher tests; 152 integration/adversarial including the
  new real-audio transcribe-and-match test)
- `cargo clippy --all-targets` — exit 0; no warnings in any new/changed code
  (remaining warnings pre-date this change)
- `./target/release/phonecheck --validate` against the updated real `.env` —
  exit 0
- No unrelated test was weakened. Config tests were updated only where they
  pinned the renamed variable and the removed lowercasing behavior (expected
  text is now stored verbatim; the matcher normalizes).

## Commit

`37ec896` on `master` (not pushed).
