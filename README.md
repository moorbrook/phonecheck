# PhoneCheck

A PBX health monitoring tool that periodically calls a phone number via SIP/VoIP, captures the audio greeting, and uses Wav2Vec2 audio embeddings for semantic matching. Sends push notifications if the expected greeting is not detected.

## Voice AI Building Blocks

This project implements many core components needed for voice AI phone applications:

- **SIP/VoIP Client** - Outbound calling with digest authentication (RFC 3261, 2617)
- **RTP Audio Handling** - Packet reception, jitter buffer, sequence ordering
- **G.711 Codec** - μ-law/A-law decoding with ITU-T compliant lookup tables
- **Audio Resampling** - 8kHz → 16kHz conversion for ML model compatibility
- **NAT Traversal** - STUN discovery + RTP hole punching for reliable audio behind NAT
- **Audio Embeddings** - Wav2Vec2 via ONNX Runtime (statically linked) for semantic audio matching
- **Speech Recognition** - Whisper integration for transcription logging
- **Formal Verification** - Kani proofs and Stateright models for correctness

## Use Case

Monitor your business phone system to ensure callers hear the correct greeting. PhoneCheck will:

1. Call your phone number every hour during business hours (8am-5pm Pacific)
2. Capture the audio and compute a semantic embedding using Wav2Vec2
3. Compare against a reference embedding using cosine similarity
4. Send you a push notification if the greeting doesn't match

## Why Audio Embeddings?

Traditional text-based matching fails when:
- Whisper transcribes "thanks for calling" but you expected "thank you for calling"
- Minor audio variations cause different transcriptions

**Wav2Vec2 embeddings solve this** by comparing audio semantically. Similar-sounding phrases produce similar embeddings, so "thanks for calling" and "thank you for calling" both match.

## Requirements

- Rust 1.88+ (ONNX Runtime is statically linked - no separate install needed)
- CMake (for building whisper.cpp)
- Python 3.11-3.13 with `uv` (one-time, for exporting Wav2Vec2 model to ONNX)
- A [voip.ms](https://voip.ms) account with a SIP sub-account for making calls
- A [Pushover](https://pushover.net) account for push notifications

## Installation

```bash
# Clone the repository
git clone https://github.com/yourusername/phonecheck.git
cd phonecheck

# Build (requires cmake; ONNX Runtime is downloaded automatically)
cargo build --release

# Download Whisper model and verify
mkdir -p models
curl -L -o models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin
echo "a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002  models/ggml-base.en.bin" | shasum -a 256 -c

# Export Wav2Vec2 to ONNX (one-time setup, downloads ~380MB model)
uv run --python 3.13 scripts/export_wav2vec2.py
```

The first build downloads ONNX Runtime (~50MB) and statically links it into the binary. The resulting binary is self-contained (~27MB) with no runtime dependencies.

## voip.ms Setup

PhoneCheck requires a [voip.ms](https://voip.ms) account for SIP calling:

1. Log in to [voip.ms portal](https://voip.ms)
2. Go to **Main Menu → Sub Accounts → Create Sub Account**
3. Choose a username and password
4. Note the assigned SIP server (e.g., `atlanta.voip.ms`)
5. Set **CallerID** to a DID you own (required for outbound calls)

## Pushover Setup

PhoneCheck uses [Pushover](https://pushover.net) for push notifications ($5 one-time purchase):

1. Create account at [pushover.net](https://pushover.net)
2. Install Pushover app on your phone
3. Copy your **User Key** from the dashboard
4. Create an application at [pushover.net/apps/build](https://pushover.net/apps/build)
5. Copy the **API Token** for your app

## Configuration

Copy `.env.example` to `.env` and configure:

```bash
cp .env.example .env
```

### Required Settings

| Variable | Description | Example |
|----------|-------------|---------|
| `SIP_USERNAME` | voip.ms sub-account username | `mysubaccount` |
| `SIP_PASSWORD` | voip.ms sub-account password | `secretpass` |
| `SIP_SERVER` | voip.ms SIP server | `atlanta.voip.ms` |
| `TARGET_PHONE` | Phone number to call | `19095551234` |
| `EXPECTED_PHRASE` | Phrase for logging (matching uses embeddings) | `thank you for calling` |
| `PUSHOVER_USER_KEY` | Your Pushover user key | `uQiRzpo4DXghDmr9QzzfQu27cmVRsG` |
| `PUSHOVER_API_TOKEN` | Your Pushover app API token | `azGDORePK8gMaC0QOYAMyEEuzJnyUi` |
| `WHISPER_MODEL_PATH` | Path to Whisper model | `./models/ggml-base.en.bin` |

### Optional Settings

| Variable | Description | Default |
|----------|-------------|---------|
| `SIP_PORT` | SIP server port | `5060` |
| `LISTEN_DURATION_SECS` | How long to listen (1-300) | `10` |
| `MIN_AUDIO_DURATION_MS` | Minimum valid audio length | `500` |
| `STUN_SERVER` | STUN server for NAT traversal | (disabled) |
| `HEALTH_PORT` | HTTP health check port | (disabled) |
| `RUST_LOG` | Log level | `info` |

## Usage

### Run as Daemon (Recommended)

```bash
# Runs hourly checks during business hours (8am-5pm Pacific)
./target/release/phonecheck
```

### Run Single Check

```bash
# Run one check immediately and exit
./target/release/phonecheck --once
```

### Capture Audio for Testing

```bash
# Save captured audio to a WAV file
./target/release/phonecheck --once --save-audio test.wav
```

### Validate Configuration

```bash
# Check that all settings are valid without making a call
./target/release/phonecheck --validate
```

### Command Line Options

```
USAGE: phonecheck [OPTIONS]

OPTIONS:
    --once                  Run a single check and exit
    --save-audio [PATH]     Save captured audio to WAV file
    --validate              Validate configuration and exit
    --help, -h              Show help message

ENVIRONMENT:
    See .env.example for required configuration variables
```

## Health Check Endpoint

If `HEALTH_PORT` is set, an HTTP server exposes:

| Endpoint | Description |
|----------|-------------|
| `GET /health` | JSON status of last check |
| `GET /ready` | 200 if healthy, 503 if last check failed |
| `GET /metrics` | Prometheus metrics |

Example:
```bash
curl http://localhost:8080/health
```

## How It Works

```
┌─────────────┐    SIP INVITE    ┌─────────────┐    PSTN    ┌─────────────┐
│ PhoneCheck  │ ───────────────► │  voip.ms    │ ─────────► │ Your Phone  │
│             │ ◄─────────────── │  Server     │ ◄───────── │   System    │
│             │    RTP Audio     │             │            │             │
└─────────────┘                  └─────────────┘            └─────────────┘
       │
       │ G.711 decode → Resample 8k→16k
       │
       ▼
┌─────────────────────────────────────────────────────────┐
│                    Audio Processing                      │
├─────────────────────────┬───────────────────────────────┤
│      Whisper            │         Wav2Vec2              │
│   (Transcription)       │       (Embeddings)            │
│                         │                               │
│  "Thank you for         │  [0.009, 0.007, -0.003, ...]  │
│   calling Cubic..."     │       768 dimensions          │
└─────────────────────────┴───────────────────────────────┘
                                    │
                                    ▼
                          ┌─────────────────┐
                          │ Cosine Similarity│
                          │ vs Reference     │
                          │                  │
                          │ similarity: 0.99 │
                          └────────┬────────┘
                                   │
                          ┌────────┴────────┐
                          │                 │
                    ≥ 0.80              < 0.80
                          │                 │
                          ▼                 ▼
                     (no action)      Send Push Alert
```

## Audio Matching

PhoneCheck uses Wav2Vec2 embeddings for semantic audio matching:

### How It Works

1. **First run**: Captures audio and saves the embedding as a reference
2. **Subsequent runs**: Compares new audio embedding against reference
3. **Threshold**: 0.80 cosine similarity (configurable in code)

### Why This Works

Wav2Vec2 embeddings capture both **phonetic** and **semantic** audio features:

| Scenario | Text Matching | Embedding Matching |
|----------|---------------|-------------------|
| "thanks for calling" vs "thank you for calling" | ❌ Fails | ✅ ~0.95 similarity |
| Same greeting, different day | ✅ Works | ✅ ~0.99 similarity |
| Wrong number / different greeting | ✅ Fails | ✅ ~0.3 similarity |

### Model Files

```
models/
├── ggml-base.en.bin          # Whisper (147MB) - transcription
├── wav2vec2_encoder.onnx     # Wav2Vec2 (1.5MB) - embeddings
├── wav2vec2_encoder.onnx.data # Wav2Vec2 weights (377MB)
└── reference_embedding.bin   # Cached reference (3KB)
```

**SHA256 Checksums:**
```
a03779c86df3323075f5e796cb2ce5029f00ec8869eee3fdfb897afe36c6d002  ggml-base.en.bin
c7c1889bdbad143221dead8137d067b092fa3adb891c76a64d26d3dcb3c41b60  wav2vec2_encoder.onnx
836b7752b6f486fb53c0c16a09342859f24d7a89d4a4eccb1818a7d31c467f27  wav2vec2_encoder.onnx.data
```

### Resetting the Reference

To capture a new reference embedding:

```bash
rm models/reference_embedding.bin
./target/release/phonecheck --once
```

## NAT Traversal

PhoneCheck works behind NAT without port forwarding using two techniques:

1. **STUN Discovery**: Queries a STUN server to learn your public IP address, which is advertised in the SDP so the remote VoIP server knows where to send audio

2. **NAT Hole Punching**: Sends empty RTP packets to the remote media server immediately after call connect, creating a NAT mapping that allows return traffic through

Both are required:
- Without STUN: The SDP advertises your private IP (192.168.x.x) which the remote can't reach
- Without hole punching: The NAT may block incoming RTP even with the correct public IP

Configure STUN in `.env`:
```bash
STUN_SERVER=stun.l.google.com:19302
```

## Logging

Control log verbosity with `RUST_LOG`:

```bash
RUST_LOG=debug ./target/release/phonecheck --once  # Verbose
RUST_LOG=warn ./target/release/phonecheck          # Quiet
```

## Troubleshooting

### "No audio received"
- Check that your SIP credentials are correct
- Verify the target phone number answers (not voicemail)
- Try increasing `LISTEN_DURATION_SECS`

### Low similarity score but greeting sounds correct
- Delete `models/reference_embedding.bin` and re-run to capture new reference
- Check audio quality with `--save-audio test.wav`
- Ensure consistent audio duration (greeting should play fully)

### "Wav2Vec2 embedder not available"
- Run `uv run --python 3.13 scripts/export_wav2vec2.py` to export the model
- Verify `models/wav2vec2_encoder.onnx` and `models/wav2vec2_encoder.onnx.data` exist

### NAT/Firewall issues
- Set `STUN_SERVER=stun.l.google.com:19302` (required for NAT traversal)
- PhoneCheck uses STUN + NAT hole punching for reliable audio behind NAT
- No port forwarding required in most cases

### Pushover alerts not sending
- Verify `PUSHOVER_USER_KEY` and `PUSHOVER_API_TOKEN` are correct
- Check Pushover app is installed on your device
- Ensure notifications are enabled for the Pushover app

## License

MIT
