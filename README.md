# PhoneCheck

A PBX health monitoring tool that periodically calls a phone number via SIP/VoIP, captures the audio greeting, transcribes it with Apple's SpeechAnalyzer, and fuzzy-matches the transcript against the expected greeting text. Sends push notifications via Pushover if the expected greeting is not detected.

## Voice AI Building Blocks

This project implements many core components needed for voice AI phone applications:

- **SIP/VoIP Client** - Outbound calling with digest authentication (RFC 3261, 2617)
- **RTP Audio Handling** - Packet reception, jitter buffer, sequence reordering
- **G.711 Codec** - μ-law/A-law decoding with ITU-T compliant lookup tables
- **Audio Resampling** - FFT-based 8kHz → 16kHz conversion using Rubato
- **NAT Traversal** - STUN discovery + RTP hole punching for reliable audio behind NAT
- **Speech Recognition** - Apple SpeechAnalyzer (macOS 26 Speech framework) via a small Swift helper subprocess — no bundled model
- **Greeting Matching** - normalized token-level similarity between transcript and expected text (no ML models, no external deps)
- **Formal Verification** - Kani proofs and Stateright models for correctness

## Architecture

PhoneCheck is built as a modular system with clearly separated concerns:

- **Orchestrator**: Manages the lifecycle of a check (INVITE, RTP capture, transcription, matching, alerting).
- **SIP Stack**: Custom implementation of RFC 3261/2617 handling registration-less outbound calls.
- **RTP Engine**: Receives G.711 packets, manages a jitter buffer for reordering, and handles NAT hole punching.
- **Speech Pipeline**: Decodes audio, resamples to 16kHz, transcribes via Apple's SpeechAnalyzer, and scores the transcript against `EXPECTED_GREETING` with token-level similarity.
- **Scheduler**: A business-hours-aware loop (8am-5pm Pacific) that manages check timing and graceful shutdown.
- **Health Server**: An embedded HTTP server providing monitoring endpoints for Kubernetes or external probes.

## Use Case

Monitor your business phone system to ensure callers hear the correct greeting. PhoneCheck will:

1. Call your phone number every hour during business hours (8am-5pm Pacific)
2. Capture the audio and transcribe it with Apple's SpeechAnalyzer
3. Compare the transcript against your expected greeting text (fuzzy, word-level)
4. Send you a push notification — including expected vs. heard text — if the greeting doesn't match or the call fails

## How Matching Works

Both the transcript and `EXPECTED_GREETING` are normalized (lowercased, punctuation stripped, whitespace collapsed) and compared as word sequences using token-level Levenshtein similarity, so one misheard word costs one edit regardless of length. Two alignments are scored and the better one wins:

- **Expected-inside-transcript**: the full expected text found anywhere in the transcript (handles captures longer than the expected text).
- **Truncated capture**: the transcript matches a leading prefix of the expected text, as long as it covers at least half of it (so hearing just "thank you" never passes).

This tolerates typical ASR jitter on 8 kHz G.711 telephone audio ("parties" for "party's", a dropped word) while rejecting wrong greetings, carrier intercept messages, and dead air. Every alert includes the expected text, the heard transcript, and the score, so misfires are easy to diagnose.

## Requirements

- macOS 26+ (transcription uses the Speech framework's SpeechAnalyzer engine)
- Rust 1.88+
- Xcode command line tools (`swiftc`, for the transcription helper)
- A [voip.ms](https://voip.ms) account with a SIP sub-account
- A [Pushover](https://pushover.net) account for notifications

## Installation

```bash
# Clone the repository
git clone https://github.com/yourusername/phonecheck.git
cd phonecheck

# Build (also compiles the SpeechAnalyzer helper via swiftc)
cargo build --release
```

The resulting binary is self-contained — no ML models to download or bundle.
Transcription needs no bundled model: the en-US speech model is an OS-managed
system asset (installed once by macOS on first use via AssetInventory).

## Configuration

Copy `.env.example` to `.env` and configure:

### Required Settings

| Variable | Description | Example |
|----------|-------------|---------|
| `SIP_USERNAME` | voip.ms sub-account username | `mysubaccount` |
| `SIP_PASSWORD` | voip.ms sub-account password | `secretpass` |
| `SIP_SERVER` | voip.ms SIP server | `atlanta.voip.ms` |
| `TARGET_PHONE` | 10-digit phone number to call | `19095551234` |
| `PUSHOVER_USER_KEY` | Your Pushover user key | `uQiRzpo4DXghD...` |
| `PUSHOVER_API_TOKEN` | Your Pushover app API token | `azGDORePK8gMa...` |

### Optional Settings

| Variable | Description | Default |
|----------|-------------|---------|
| `EXPECTED_GREETING` | Expected greeting text (word-level fuzzy match) | `thank you for calling cubic machinery` |
| `GREETING_MATCH_THRESHOLD` | Similarity threshold in (0.0, 1.0] | `0.75` |
| `SIP_PORT` | SIP server port | `5060` |
| `LISTEN_DURATION_SECS` | How long to listen (max 300) | `10` |
| `MIN_AUDIO_DURATION_MS`| Min audio needed to avoid silence alerts | `500` |
| `STUN_SERVER` | STUN server for NAT (e.g. `stun.l.google.com:19302`) | (disabled) |
| `HEALTH_PORT` | HTTP health check port | (disabled) |
| `SPEECH_HELPER_PATH` | Path to SpeechAnalyzer helper binary | `./native/speech_helper` |
| `RUST_LOG` | Log level (error, warn, info, debug, trace) | `info` |

## Usage

### Run as Daemon
Runs hourly checks during business hours (8am-5pm Pacific).
```bash
./target/release/phonecheck
```

### Run Single Check
```bash
./target/release/phonecheck --once
```

### Advanced Flags
- `--validate`: Check configuration and network reachability without calling.
- `--save-audio [path]`: Save the captured audio to a WAV file for debugging.

## Advanced Features

### Formal Verification
PhoneCheck uses advanced verification techniques to ensure reliability:
- **Kani Proofs**: Formally verify that PII redaction (phones/emails) never leaks data and that RTP header parsing is memory-safe.
- **Stateright Models**: Model the SIP state machine and Scheduler logic to prove absence of deadlocks and correct state transitions.

### NAT Traversal
Works behind NAT without port forwarding by combining:
1. **STUN Discovery**: Learns public IP to advertise in SIP SDP.
2. **RTP Hole Punching**: Sends empty packets to the remote server to open the NAT mapping for return audio.

### Graceful Shutdown
Handles `SIGINT` (Ctrl+C) and `SIGTERM` cleanly:
- Active calls are terminated with a SIP `BYE` message.
- The scheduler waits up to 10 seconds for in-flight tasks to complete.
- Singleton lock (`/tmp/phonecheck.lock`) is released automatically.

### Health Monitoring
If `HEALTH_PORT` is set, an HTTP server exposes:
- `GET /health`: JSON status including success/failure counts and timestamps.
- `GET /ready`: Returns 200 if the last check succeeded, 503 if it failed.
- `GET /metrics`: Prometheus-compatible metrics for integration with Grafana.

## Tuning the Match

At the default threshold of **0.75**:
- A clean transcript of the right greeting scores **1.0**.
- Typical ASR jitter (one or two misheard/dropped words in eleven) scores **0.82-0.91**.
- A capture covering only half the expected text still passes if the words are right.
- Wrong greetings and carrier intercept ("not in service") messages score **<0.4**.

If checks fail with a plausible transcript in the alert, extend or correct `EXPECTED_GREETING` to match what is actually heard within `LISTEN_DURATION_SECS`, rather than lowering the threshold.

## Troubleshooting

- **No audio**: Ensure `STUN_SERVER` is configured if you are behind NAT.
- **Low similarity**: The alert shows expected vs. heard text — if the greeting is cut off, increase `LISTEN_DURATION_SECS` or shorten `EXPECTED_GREETING`.
- **Stale lock**: If the process crashed, manually remove `/tmp/phonecheck.lock`.

## License

MIT
