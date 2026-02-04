# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

PhoneCheck is a PBX health monitoring tool that periodically calls a phone number via SIP/VoIP, captures the audio greeting, and uses Wav2Vec2 audio embeddings for semantic matching. Sends push notifications via Pushover if the expected greeting is not detected.

## Build Commands

```bash
# Build (requires cmake for whisper.cpp; ONNX Runtime is statically linked)
cargo build --release

# Run tests
cargo test

# Run single check (for testing)
./target/release/phonecheck --once

# Run as daemon (scheduled hourly, 8am-5pm Pacific)
./target/release/phonecheck

# Validate configuration without making a call
./target/release/phonecheck --validate
```

## Architecture

```
src/
├── main.rs          # Entry point, CLI args, orchestration, singleton lock
├── config.rs        # Environment variable loading, validation
├── scheduler.rs     # Business hours scheduling (8am-5pm Pacific), graceful shutdown
├── redact.rs        # PII redaction for logging (phone numbers, emails, SIP URIs)
├── embedding.rs     # Wav2Vec2 audio embeddings for semantic matching
├── health.rs        # HTTP health check server (/health, /ready, /metrics)
├── stun.rs          # STUN client for NAT traversal (RFC 5389)
├── notify.rs        # Pushover push notification integration
├── sip/             # SIP protocol implementation
│   ├── mod.rs       # Module exports
│   ├── client.rs    # SIP UAC (outbound call logic, error classification)
│   ├── digest.rs    # RFC 2617/7616 digest authentication
│   ├── messages.rs  # SIP message building (INVITE, ACK, BYE, SDP)
│   └── model.rs     # Stateright model for state machine verification
└── rtp/             # RTP audio handling
    ├── mod.rs       # Module exports, WAV file saving utilities
    ├── receiver.rs  # RTP packet reception, jitter buffer, reordering
    ├── jitter.rs    # Jitter buffer for handling packet loss/reordering
    └── g711.rs      # G.711 u-law/A-law codec (ITU-T lookup tables)
```

## Key Data Flow

1. **SIP INVITE** → voip.ms server → target phone number (with digest auth if required)
2. **RTP audio** ← G.711 encoded @ 8kHz (PCMU/PCMA)
3. **NAT Traversal** → STUN discovery + RTP hole punching for audio behind NAT
4. **Decode** → PCM i16 → jitter buffer reordering
5. **Resample** → 8kHz → 16kHz f32 (Rubato FFT-based)
6. **Whisper** → transcribe for logging/debugging
7. **Wav2Vec2** → compute 768-dimensional embedding (mean pooled, L2 normalized)
8. **Match** → cosine similarity vs reference embedding (threshold: 0.75)
9. **Alert** → Pushover push notification if similarity < 0.75

## Configuration

Copy `.env.example` to `.env` and configure:

### Required
- **SIP credentials**: voip.ms sub-account (SIP_USERNAME, SIP_PASSWORD, SIP_SERVER)
- **Target phone**: TARGET_PHONE (10 digits for voip.ms)
- **Pushover**: PUSHOVER_USER_KEY, PUSHOVER_API_TOKEN
- **Models**: WHISPER_MODEL_PATH (ggml-base.en.bin from HuggingFace)

### Optional
- **Audio settings**: LISTEN_DURATION_SECS (default: 10), MIN_AUDIO_DURATION_MS (default: 500)
- **STUN**: STUN_SERVER (e.g., stun.l.google.com:19302) for NAT traversal
- **Health server**: HEALTH_PORT (exposes /health, /ready, /metrics endpoints)
- **Logging**: RUST_LOG (default: info)

### Important Notes
- **Similarity threshold is hardcoded at 0.75** - do not make this configurable to avoid option paralysis
- First successful check captures and caches the reference embedding to `models/reference_embedding.bin`
- Delete reference_embedding.bin to capture a new baseline

## Dependencies

- **whisper-rs**: Requires `cmake` to build whisper.cpp
- **ort (ONNX Runtime)**: Statically linked (~50MB), no runtime dependency
- **Rubato**: FFT-based audio resampling
- G.711 lookup tables sourced from [zaf/g711](https://github.com/zaf/g711)

## Testing

- **Unit tests**: Comprehensive coverage of all modules
- **Property-based tests**: proptest for config parsing, redaction, resampling, SIP errors
- **Snapshot tests**: insta for embedding similarity decisions and audio matching
- **Formal verification**: Kani proofs for critical invariants (redaction doesn't leak PII, RTP header parsing)
- **State machine models**: Stateright for scheduler and health metrics

Run `cargo test` for all tests.

## Lock File

PhoneCheck uses a singleton lock file (`/tmp/phonecheck.lock`) to prevent concurrent instances:
- Acquired at startup via `try_lock_exclusive()` with fs2 crate
- Held for the entire process lifetime
- Released automatically on normal exit (drop)

**Abnormal exit (SIGKILL, system crash):** The lock file may not be released.
If you see "Another instance of phonecheck is already running" but no process is running:

```bash
# Check if phonecheck is actually running
ps aux | grep phonecheck

# If not running, manually remove the lock file
rm /tmp/phonecheck.lock
```

## NAT Traversal

PhoneCheck works behind NAT without port forwarding:

1. **STUN Discovery** (stun.rs): Queries a STUN server to learn your public IP address
   - Public IP is advertised in SDP so remote VoIP server knows where to send audio
   - Without STUN: SDP contains private IP (192.168.x.x) which remote can't reach

2. **NAT Hole Punching** (rtp/receiver.rs): Sends empty RTP packets to remote media server
   - Creates NAT mapping allowing return traffic on the same port
   - Without hole punching: NAT blocks incoming RTP even with correct public IP

Both techniques are required for reliable audio behind NAT.

## Audio Matching Details

**Wav2Vec2 embeddings** are used instead of text matching because:
- Whisper transcription varies: "thanks for calling" vs "thank you for calling"
- Embeddings capture phonetic and semantic features
- Cosine similarity handles minor audio variations

**Duration sensitivity:** Mean-pooled embeddings vary with audio duration:
- 1 second capture vs 5 second reference: ~0.79 similarity
- 2 second capture vs 5 second reference: ~0.91 similarity
- Full capture vs reference: ~0.99 similarity

The hardcoded 0.75 threshold accommodates these variations while rejecting truly different content (~0.02 similarity).

## Health Check Server

If `HEALTH_PORT` is set, an HTTP server exposes monitoring endpoints:
- `GET /health` - JSON status with success/failure counts, last check time
- `GET /ready` - 200 if last check succeeded or no checks yet, 503 if last check failed
- `GET /metrics` - Prometheus-compatible metrics format

Useful for Kubernetes probes, load balancer health checks, or external monitoring.

## Graceful Shutdown

PhoneCheck handles shutdown signals (SIGINT, SIGTERM) gracefully:
- Scheduler loop checks for cancellation token between checks
- Active SIP calls receive BYE to end cleanly (10 second timeout)
- Health server shuts down on cancellation
- RTP receiver supports cancellation mid-stream
- In-progress embeddings/transcription complete before exit

## SIP Authentication

SIP digest authentication (RFC 2617/7616) is fully implemented:
- On 401/407 response, extracts WWW-Authenticate header
- Computes MD5 digest response with username/password, method, and URI
- Retries INVITE with Authorization header
- Handles both standard and qop (quality of protection) challenges

Set `SIP_PASSWORD` in .env for providers requiring authentication.
