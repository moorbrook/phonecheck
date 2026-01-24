# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

PhoneCheck is a PBX health monitoring tool that periodically calls a phone number via SIP/VoIP, transcribes the audio greeting using Whisper, and sends SMS alerts if the expected phrase is not detected.

## Build Commands

```bash
# Build (requires cmake for whisper.cpp)
cargo build --release

# Run tests
cargo test

# Run single check (for testing)
./target/release/phonecheck --once

# Run as daemon (scheduled hourly, 8am-5pm Pacific)
./target/release/phonecheck
```

## Architecture

```
src/
├── main.rs          # Entry point, CLI args, orchestration
├── config.rs        # Environment variable loading
├── scheduler.rs     # Business hours scheduling (8am-5pm Pacific)
├── sip/             # SIP protocol implementation
│   ├── client.rs    # SIP UAC (outbound call logic)
│   ├── messages.rs  # SIP message building (INVITE, ACK, BYE)
│   └── transport.rs # UDP transport layer
├── rtp/             # RTP audio handling
│   ├── receiver.rs  # RTP packet reception and reassembly
│   └── g711.rs      # G.711 u-law/A-law codec (lookup tables from ITU-T spec)
├── speech.rs        # Whisper transcription + phrase matching
└── notify.rs        # voip.ms SMS API integration
```

## Key Data Flow

1. **SIP INVITE** → voip.ms server → target phone number
2. **RTP audio** ← G.711 encoded @ 8kHz
3. **Decode** → PCM i16 → resample to 16kHz f32
4. **Whisper** → transcribe → fuzzy match expected phrase
5. **Alert** → voip.ms SMS API if phrase not found

## Configuration

Copy `.env.example` to `.env` and configure:
- SIP credentials (voip.ms sub-account)
- Target phone number
- Expected phrase
- SMS alert settings
- Whisper model path (download GGML models from HuggingFace)

## Dependencies

- **whisper-rs**: Requires `cmake` to build whisper.cpp
- G.711 lookup tables sourced from [zaf/g711](https://github.com/zaf/g711)

## Testing

- G.711 codec has property-based tests (proptest)
- SIP message building has unit tests with RFC 3261 compliance
- Run `cargo test` for all tests

## Notes

- Audio resampling uses simple linear interpolation (8kHz → 16kHz)
- Fuzzy phrase matching allows 1 Levenshtein distance per word
- SIP password is loaded but authentication not yet implemented (voip.ms may not require it for some configurations)
