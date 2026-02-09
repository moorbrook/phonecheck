# PhoneCheck

A PBX health monitoring tool that periodically calls a phone number via SIP/VoIP, captures the audio greeting, and uses Wav2Vec2 audio embeddings for semantic matching. Sends push notifications via Pushover if the expected greeting is not detected.

## Voice AI Building Blocks

This project implements many core components needed for voice AI phone applications:

- **SIP/VoIP Client** - Outbound calling with digest authentication (RFC 3261, 2617)
- **RTP Audio Handling** - Packet reception, jitter buffer, sequence reordering
- **G.711 Codec** - μ-law/A-law decoding with ITU-T compliant lookup tables
- **Audio Resampling** - FFT-based 8kHz → 16kHz conversion using Rubato
- **NAT Traversal** - STUN discovery + RTP hole punching for reliable audio behind NAT
- **Audio Embeddings** - Wav2Vec2 via ONNX Runtime (statically linked) for semantic matching
- **Speech Recognition** - Whisper integration for transcription logging
- **Formal Verification** - Kani proofs and Stateright models for correctness

## Architecture

PhoneCheck is built as a modular system with clearly separated concerns:

- **Orchestrator**: Manages the lifecycle of a check (INVITE, RTP capture, ML processing, Alerting).
- **SIP Stack**: Custom implementation of RFC 3261/2617 handling registration-less outbound calls.
- **RTP Engine**: Receives G.711 packets, manages a jitter buffer for reordering, and handles NAT hole punching.
- **ML Pipeline**: Decodes audio, resamples to 16kHz, transcribes via Whisper (for logs), and computes Wav2Vec2 embeddings for comparison.
- **Scheduler**: A business-hours-aware loop (8am-5pm Pacific) that manages check timing and graceful shutdown.
- **Health Server**: An embedded HTTP server providing monitoring endpoints for Kubernetes or external probes.

## Use Case

Monitor your business phone system to ensure callers hear the correct greeting. PhoneCheck will:

1. Call your phone number every hour during business hours (8am-5pm Pacific)
2. Capture the audio and compute a semantic embedding using Wav2Vec2
3. Compare against a reference embedding using cosine similarity
4. Send you a push notification if the greeting doesn't match or the call fails

## Why Audio Embeddings?

Traditional text-based matching fails when:
- Whisper transcribes "thanks for calling" but you expected "thank you for calling"
- Minor audio variations cause different transcriptions (e.g., background noise)

**Wav2Vec2 embeddings solve this** by comparing audio semantically. Similar-sounding phrases produce similar embeddings, allowing for natural variation while detecting significant changes or failures.

## Requirements

- Rust 1.88+
- CMake (for building whisper.cpp)
- Python 3.11-3.13 with `uv` (one-time setup for exporting Wav2Vec2 model)
- A [voip.ms](https://voip.ms) account with a SIP sub-account
- A [Pushover](https://pushover.net) account for notifications

## Installation

```bash
# Clone the repository
git clone https://github.com/yourusername/phonecheck.git
cd phonecheck

# Build (requires cmake; ONNX Runtime is downloaded automatically)
cargo build --release

# Download Whisper model
mkdir -p models
curl -L -o models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin

# Export Wav2Vec2 to ONNX (downloads ~380MB model)
uv run scripts/export_wav2vec2.py
```

The resulting binary is self-contained (~27MB) as ONNX Runtime is statically linked.

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
| `EXPECTED_PHRASE` | Phrase for logging/transcription check | `thank you for calling` |
| `SIP_PORT` | SIP server port | `5060` |
| `LISTEN_DURATION_SECS` | How long to listen (max 300) | `10` |
| `MIN_AUDIO_DURATION_MS`| Min audio needed to avoid silence alerts | `500` |
| `STUN_SERVER` | STUN server for NAT (e.g. `stun.l.google.com:19302`) | (disabled) |
| `HEALTH_PORT` | HTTP health check port | (disabled) |
| `WHISPER_MODEL_PATH` | Path to Whisper GGML model | `./models/ggml-base.en.bin` |
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

## Audio Matching

### Reference Capture
On the first successful run, PhoneCheck saves the audio embedding as a baseline in `models/reference_embedding.bin`. Subsequent runs compare against this baseline.

### Similarity Threshold
The cosine similarity threshold is hardcoded at **0.75**. 
- Same greeting typically yields **>0.95**.
- Slight variations (duration/noise) yield **0.80-0.90**.
- Different greetings or "number not in service" messages yield **<0.10**.

To reset the baseline:
```bash
rm models/reference_embedding.bin
./target/release/phonecheck --once
```

## Troubleshooting

- **No audio**: Ensure `STUN_SERVER` is configured if you are behind NAT.
- **Low similarity**: If the greeting is cut off, increase `LISTEN_DURATION_SECS`.
- **Stale lock**: If the process crashed, manually remove `/tmp/phonecheck.lock`.

## License

MIT
