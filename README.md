# PhoneCheck

A PBX health monitoring tool that periodically calls a phone number via SIP/VoIP, transcribes the audio greeting using Whisper, and sends push notifications if the expected phrase is not detected.

## Use Case

Monitor your business phone system to ensure callers hear the correct greeting. PhoneCheck will:

1. Call your phone number every hour during business hours (8am-5pm Pacific)
2. Listen to the greeting and transcribe it using Whisper AI
3. Check if the expected phrase is present (fuzzy matching allows minor variations)
4. Send you a push notification if something is wrong

## Requirements

- Rust 1.70+
- CMake (for building whisper.cpp)
- A [voip.ms](https://voip.ms) account with a SIP sub-account for making calls
- A [Pushover](https://pushover.net) account for push notifications

## Installation

```bash
# Clone the repository
git clone https://github.com/yourusername/phonecheck.git
cd phonecheck

# Build (requires cmake)
cargo build --release

# Download a Whisper model
mkdir -p models
curl -L -o models/ggml-base.en.bin \
  https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.en.bin
```

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
| `EXPECTED_PHRASE` | Phrase to detect in greeting | `thank you for calling` |
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

### Validate Configuration

```bash
# Check that all settings are valid without making a call
./target/release/phonecheck --validate
```

### Command Line Options

```
USAGE: phonecheck [OPTIONS]

OPTIONS:
    --once              Run a single check and exit
    --validate          Validate configuration and exit
    --help, -h          Show help message

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
       │ G.711 decode → Resample → Whisper transcribe
       │
       ▼
┌─────────────┐
│ "Thank you  │──── Fuzzy match ────► Expected phrase found?
│ for calling │                              │
│ Acme Corp"  │                     ┌────────┴────────┐
└─────────────┘                     │                 │
                                   YES               NO
                                    │                 │
                                    ▼                 ▼
                               (no action)      Send Push Alert
```

## Phrase Matching

PhoneCheck uses Levenshtein distance (edit distance) for fuzzy matching:

- Case insensitive
- Allows 1 character difference per word (for words > 3 characters)
- Words must appear in order

Examples that match `"thank you for calling"`:
- `"Thank you for calling"` (case difference)
- `"thanks you for calling"` (1 char difference in "thank")
- `"thank you for calling Acme Corp"` (extra words OK)

### Algorithm Tradeoffs

We evaluated several matching approaches:

| Algorithm | Pros | Cons | Best For |
|-----------|------|------|----------|
| **Levenshtein** (current) | Simple, fast, deterministic, no dependencies | Only catches typos, not synonyms | Transcription errors like `"machinary"` |
| **Jaro-Winkler** | Good for prefix similarity, scores 0-1 | Weights prefixes heavily, may false-positive on short words | Name matching, abbreviations |
| **Soundex** | Phonetic matching | Too coarse, groups dissimilar words | Spelling variations of names |
| **Word2Vec/GloVe** | Semantic word similarity | Word-level only, requires ~100-300MB model | Synonym matching |
| **Sentence-BERT** | Full sentence semantics, handles paraphrasing | Requires ~80-400MB model + ML runtime | Intent matching, variable phrasings |
| **LLM** | Best semantic understanding | Slow, expensive, requires API or large model | Complex intent classification |

**Why Levenshtein?** Whisper transcription errors are typically character-level typos (`"machinery"` → `"machinary"`), not semantic variations. The greeting is scripted and consistent, so we don't need synonym or paraphrase matching. Levenshtein is fast, has no dependencies, and handles actual transcription errors well.

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

### "Phrase not found" but greeting is correct
- Check the Whisper transcription in debug logs
- Try a larger Whisper model (`ggml-small.en.bin`)
- Adjust `EXPECTED_PHRASE` to match what Whisper hears

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
