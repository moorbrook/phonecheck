# PhoneCheck

A PBX health monitoring tool that periodically calls a phone number via SIP/VoIP, transcribes the audio greeting using Whisper, and sends SMS alerts if the expected phrase is not detected.

## Use Case

Monitor your business phone system to ensure callers hear the correct greeting. PhoneCheck will:

1. Call your phone number every hour during business hours (8am-5pm Pacific)
2. Listen to the greeting and transcribe it using Whisper AI
3. Check if the expected phrase is present (fuzzy matching allows minor variations)
4. Send you an SMS alert if something is wrong

## Requirements

- Rust 1.70+
- CMake (for building whisper.cpp)
- A [voip.ms](https://voip.ms) account with:
  - A SIP sub-account for making calls
  - SMS-enabled DID for sending alerts
  - API access enabled

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

PhoneCheck requires a [voip.ms](https://voip.ms) account. You'll need to configure three things:

### 1. Create a SIP Sub-Account (for making calls)

1. Log in to [voip.ms portal](https://voip.ms)
2. Go to **Main Menu → Sub Accounts → Create Sub Account**
3. Choose a username and password
4. Note the assigned SIP server (e.g., `atlanta.voip.ms`)
5. Set **CallerID** to a DID you own (required for outbound calls)

### 2. Enable the REST API (for sending SMS alerts)

1. Go to **Main Menu → SOAP and REST/JSON API** ([direct link](https://voip.ms/m/api.php))
2. Click **Enable API** if not already enabled
3. Set an **API Password** (different from your login password) and click **Save API Password**
4. Under **IP Addresses**, either:
   - Add your server's IP address, OR
   - Enter `0.0.0.0` to allow access from any IP (less secure)
5. Click **Save IP Addresses**

Your API credentials are:
- **Username**: Your voip.ms account email
- **Password**: The API password you just set (not your login password)

See [voip.ms API documentation](https://voip.ms/resources/api) for more details.

### 3. Enable SMS on a DID (for sending alerts)

1. Go to **DID Numbers → Manage DIDs**
2. Click the **Edit** (pencil icon) button for the DID you want to use
3. Scroll to **SMS/MMS Configuration** section
4. Check **Enable SMS/MMS Service**
5. Click **Apply Changes**

Note: US business SMS requires [10DLC registration](https://wiki.voip.ms/article/SMS-MMS). Unregistered traffic may be filtered or incur additional fees.

See [voip.ms SMS/MMS documentation](https://wiki.voip.ms/article/SMS-MMS) for more details.

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
| `VOIPMS_API_USER` | voip.ms main account email | `you@email.com` |
| `VOIPMS_API_PASS` | voip.ms API password | `apipassword` |
| `VOIPMS_SMS_DID` | SMS-enabled DID | `19095559999` |
| `ALERT_PHONE` | Your cell phone for alerts | `19095558888` |
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
                               (no action)      Send SMS Alert
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

### SMS alerts not sending
- Verify `VOIPMS_API_USER` and `VOIPMS_API_PASS`
- Check that `VOIPMS_SMS_DID` has SMS enabled
- Ensure API access is enabled in voip.ms portal

## License

MIT
