# PhoneCheck — Review, Repairs, and Voice-Agent Extension Plan

Date: 2026-07-12
Scope: (A) build/test health, (B) adversarial code review with fixes, (C) research + plan for extending PhoneCheck into an LLM that answers Cubic Machinery's phones.

---

## 1. Current-state verdict

**PhoneCheck works again.** All three gates pass on the current working tree:

| Gate | Before | After |
|---|---|---|
| `cargo build --release` | OK (25 s, warnings only) | OK |
| `cargo test --release` | **FAILED** — 3 of 242 lib tests red; embedder could not load | **OK — 388/388 pass** (239 lib + 149 integration/adversarial) |
| `cargo clippy --all-targets` | **FAILED** — 8 hard errors (deny-by-default lint) | **OK — 0 errors** (~30 style warnings remain, all in test code) |

**The most serious finding was not a test failure but a production outage:** the Wav2Vec2 ONNX model (`models/wav2vec2_encoder.onnx`) stores its weights in an external file, `wav2vec2_encoder.onnx.data` (360 MB), which had been deleted sometime after 2026-02-10 (`phonecheck.log` shows the embedder loading successfully through that date). `models/` is gitignored, so git could not restore it. With the weights missing, ONNX Runtime fails in `Initialize()`, the embedder never loads, and **every real check would have reported "expected greeting not detected" and alerted** — the tool was silently broken end-to-end, not just in tests. The model was regenerated with the repo's own `scripts/export_wav2vec2.py`; both output files match the SHA256 checksums pinned in that script, so the regenerated model is byte-identical to the original export and the cached `reference_embedding.bin` remains valid (verified: full test audio vs reference similarity = 0.9977).

Context notes:

- The working tree already carried substantial uncommitted modifications across ~28 files (a previous improvement pass, last commit `882df27` on 2026-05-13). This review treats the working tree as the current state. Nothing was committed, per instructions.
- What the project is: a PBX watchdog. Hourly (8am–5pm Pacific), it places a SIP call through voip.ms to the shop's main number, captures the greeting audio over RTP (G.711 → jitter buffer → 8k→16k resample), transcribes with Whisper for logging, computes a Wav2Vec2 embedding, and compares cosine similarity against a cached reference (threshold 0.75). On mismatch or call failure it sends a Pushover alert. Includes STUN/CGNAT NAT traversal, SIP digest auth + REGISTER, a health/metrics HTTP server, and an unusually deep test suite (proptest, insta snapshots, stateright models, Kani proofs).

---

## 2. Bugs found and fixed

Each item: root cause in one sentence, then the fix. All fixes are in the working tree only (not committed).

### 2.1 Missing ONNX weights file — phrase matching completely broken *(bit-rot, severity: critical)*

**Root cause:** `wav2vec2_encoder.onnx` references external weights in `wav2vec2_encoder.onnx.data`, which had been deleted (unrecoverable — `models/` is gitignored), so the embedder failed to initialize and every check would alert.
**Fix:** regenerated via `uv run scripts/export_wav2vec2.py`; SHA256 checksums of both files verified against the values pinned in the script. No code change.

### 2.2 Graceful shutdown aborts the in-flight call instead of finishing it — `src/scheduler.rs` *(severity: high)*

**Root cause:** `tokio::select!` took the check future *by value*, so when the shutdown branch won, the future was **dropped** (the SIP call aborted mid-flight, no BYE sent), after which the code cancelled the token and slept up to 10 s "waiting" for a future that no longer existed.
**Fix:** new `await_check_with_shutdown()` helper pins the future (`tokio::pin!`) and keeps polling **the same future** after triggering cancellation, bounded by the 10 s graceful timeout, so the call's cleanup path (BYE, socket teardown) actually runs. A side effect of the old structure — a spurious `watch` wake (send of `false`) also silently dropped the check — is fixed by the same loop.
**Regression tests added:** `test_check_future_survives_shutdown_and_finishes_cleanup` (asserts the post-cancellation cleanup code runs; fails against the old structure) and `test_graceful_shutdown_timeout_bounds_stuck_check`.

### 2.3 Multi-second blocking inference on the async runtime — `src/orchestrator.rs` *(severity: medium)*

**Root cause:** Whisper transcription + ONNX embedding (seconds of CPU-bound work, executed while holding a `std::sync::Mutex`) ran directly on a tokio worker thread, starving the health server and shutdown handling for the duration.
**Fix:** `process_audio` is now wrapped in a `run_cpu_bound()` helper that uses `tokio::task::block_in_place` on the multi-thread runtime (and runs inline on a current-thread runtime, where `block_in_place` would panic — relevant only for tests).

### 2.4 RTP header-extension off-by-one decodes garbage as audio — `src/rtp/receiver.rs` *(severity: low, malformed-input handling)*

**Root cause:** the extension-skip condition `data.len() > offset + 4` (strict `>`) means a packet whose extension header ends exactly at the packet boundary (X=1, `ext_length` = 0, empty payload) is not skipped, so the 4-byte extension header is fed to the G.711 decoder as 4 garbage audio samples.
**Fix:** `>=` in both copies of the parser (`calculate_payload_offset` and the public `parse_rtp_header`). All reads remain in-bounds (`offset+2`, `offset+3` < `offset+4` ≤ `len`).
**Regression tests added:** `test_parse_rtp_header_extension_only_packet` (offset must be 16, not 12) and `test_receiver_ignores_extension_only_packet_payload` (sample buffer stays empty). The existing `test_extension_length_overflow_safe` adversarial test still passes.

### 2.5 `RUST_LOG` silently ignored for the app's own logs — `src/main.rs` *(severity: low)*

**Root cause:** `EnvFilter::from_default_env().add_directive("phonecheck=info")` unconditionally appended a target-specific directive that overrides any `RUST_LOG` setting for the `phonecheck` crate, making `RUST_LOG=debug` a no-op — contradicting the documented behavior ("Logging: RUST_LOG").
**Fix:** `EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("phonecheck=info"))` — `RUST_LOG` wins when set; `phonecheck=info` is only the fallback.

### 2.6 `cargo clippy --all-targets` hard-failed on test code *(bit-rot, severity: low)*

**Root cause:** `clippy::absurd_extreme_comparisons` (deny-by-default) fires on tautological assertions like `sample >= i16::MIN && sample <= i16::MAX` in the G.711 property tests, turning `--all-targets` clippy into a hard failure.
**Fix:** module-level `#![allow(clippy::absurd_extreme_comparisons, clippy::manual_range_contains)]` in `src/rtp/g711.rs` (proptests module) and `tests/adversarial_g711.rs`, with a comment. **No assertion was changed** — the lint is technically right that the checks are vacuous by type, but they document the intended property.

### 2.7 Stale insta snapshots of a runtime-mutable file *(test-data refresh — flagged for review)*

**Root cause:** three snapshot tests pin exact floating-point stats (min/max/mean, similarity to 4 decimals) of `models/reference_embedding.bin`, but `speech.rs` **deliberately rewrites that file** whenever a live check scores > 0.95 similarity (self-updating reference; last rewrite 2026-02-15), so those snapshots go stale by design.
**Action taken:** verified that **every boolean/decision line was unchanged** (`is_normalized: true`, `has_nan: false`, all `passes_threshold: true/false` verdicts identical; only 3rd–4th-decimal values drifted, e.g. `full_audio_vs_reference` 0.9972 → 0.9977), then refreshed the three snapshots via `cargo insta accept`. This is insta's designed data-refresh workflow, not a change to test logic — but it *is* a change to recorded expectations, so it is called out here explicitly: **revert `src/snapshots/*.snap` if you disagree.** The durable fix (recommended, not done — it would change test specifications): assert invariants (dimension, L2 norm ≈ 1, no NaN, threshold verdicts) instead of exact values of a file the app mutates.

---

## 3. Remaining concerns (reviewed, deliberately not changed)

1. **Reference-embedding drift loop.** The > 0.95 self-update in `speech.rs` slowly re-anchors the reference to whatever keeps matching. Over months a gradually degrading greeting could drag the reference with it. Consider pinning the reference and requiring manual re-baseline, or keeping the bootstrap reference alongside and alerting if the *original* similarity decays.
2. **The deleted-weights failure mode will recur.** `models/` is gitignored and `--validate` only checks the Whisper path. Recommend: `Config::validate()` should also check `wav2vec2_encoder.onnx` + `.onnx.data` existence (the export script's checksums could be verified at startup).
3. **Ringing longer than 32 s fails the call.** `send_invite_await_final` enforces Timer B (32 s) even after a provisional response; RFC 3261 stops Timer B once Proceeding. Harmless while the monitored PBX answers quickly; would matter for the voice-agent work (Section 4).
4. **Jitter-buffer final flush is not wraparound-aware.** `drain()`'s leftover path appends packets in raw `u16` order; only wrong across a sequence wraparound at capture end. Cosmetic.
5. **FFT resampler delay not flushed** (`resample.rs`): Rubato's filter delay is neither primed nor drained, so output is slightly shifted/truncated. Harmless for embedding matching; would matter for a duplex audio path.
6. **`ModelManager::get` init race** — two threads could both build a manager, one discarded (`OnceCell::set` loses). Benign single-threaded startup today.
7. **~30 clippy style warnings remain** (too-many-arguments on SIP builders, unused `ssrc` field, `unused import: super::*` in some test modules). Cosmetic; left to avoid churn in a dirty tree.
8. **`phonecheck.log` sits untracked in the repo root** and contains call transcripts/similarity history. Phone numbers are redacted by `redact.rs`, but keep it out of any commit.

---

## 4. Extending PhoneCheck into an LLM that answers Cubic's phones

### 4.1 State of the art, mid-2026

**OpenAI — two distinct tracks:**

- **GPT-Live-1 / GPT-Live-1 mini (announced July 8, 2026).** OpenAI's first true **full-duplex** voice models — they listen and speak simultaneously (natural interruptions, overlap, live translation), replacing turn-based Advanced Voice Mode in ChatGPT. Architecture is two-layer: a continuous real-time interaction layer plus a delegation layer that hands harder tasks to GPT-5.5 in the background. **Consumer-only at launch (ChatGPT iOS/Android/web); no API yet** — there is a sign-up form for API access (`openai.com/form/gpt-live-1-in-the-api`). Sources: [TechCrunch](https://techcrunch.com/2026/07/08/openai-releases-new-voice-models-for-more-natural-live-conversations/), [MLQ](https://mlq.ai/news/openai-launches-gpt-live-1-a-full-duplex-voice-model-that-listens-and-speaks-simultaneously/), [technology.org](https://www.technology.org/2026/07/09/openai-gpt-live-full-duplex-voice-models/).
- **Realtime API + gpt-realtime (GA since Aug 2025; current: gpt-realtime-2.1 / -2.1-mini).** Production speech-to-speech with server-side VAD/semantic turn detection (half-duplex with fast barge-in, not full-duplex), tool calling, MCP support, and — decisive for this project — **native SIP**: you point a SIP trunk at `sip:$PROJECT_ID@sip.api.openai.com;transport=tls`, receive a `realtime.call.incoming` webhook, then accept/reject/monitor/**refer (transfer)**/hangup via REST + WebSocket. Reported end-to-end latency 250–500 ms, with 2026 posts citing sub-300 ms typical (UNVERIFIED vendor/secondary claims). Pricing (secondary sources, July 2026): ~$32/$64 per M audio tokens in/out (≈ $0.30–0.40/min naive, ~$0.04–0.10/min with caching + VAD trimming); mini tier roughly one-third of that. Sources: [OpenAI announcement](https://openai.com/index/introducing-gpt-realtime/), [OpenAI SIP guide](https://developers.openai.com/api/docs/guides/realtime-sip), [pricing docs](https://developers.openai.com/api/docs/pricing), [HackerNoon cost data](https://hackernoon.com/openai-realtime-api-pricing-in-2026-real-world-data-from-4000-measured-sessions), [aireiter](https://aireiter.com/blog/openai-realtime-api-pricing).

**Practical takeaway:** GPT-Live-1 is the headline, but the **Realtime API with SIP is the thing you can actually wire a phone line to today.** When GPT-Live-1 reaches the API, it should slot into the same SIP/webhook plumbing.

**Competition:**

- **Google Gemini Live API** — native-audio single-model path (Gemini 2.5 Flash Native Audio; **Gemini 3.1 Flash Live**, March 2026, preview), WebSocket full-duplex transport with barge-in; no first-party SIP — telephony goes through LiveKit/Pipecat/partners. Sources: [Google blog](https://blog.google/innovation-and-ai/technology/developers-tools/build-with-gemini-3-1-flash-live/), [ai.google.dev](https://ai.google.dev/gemini-api/docs/live-api).
- **Amazon Nova Sonic / Nova 2 Sonic** — Bedrock bidirectional-streaming speech-to-speech; Nova 2 Sonic added direct integrations with Amazon Connect, Twilio, Vonage, Audiocodes, LiveKit, Pipecat, and explicitly improved **8 kHz telephony audio** handling. Sources: [AWS announcement](https://aws.amazon.com/blogs/aws/introducing-amazon-nova-2-sonic-next-generation-speech-to-speech-model-for-conversational-ai/), [telephony guide](https://aws.amazon.com/blogs/machine-learning/building-ai-powered-voice-applications-amazon-nova-sonic-telephony-integration-guide/).
- **Open models (Kyutai lineage)** — **Moshi**: the original open full-duplex speech model (~160–200 ms theoretical/practical latency) but weak at tools/reasoning; **Unmute** (2025): Kyutai's modular cascade that wraps *any* text LLM with their streaming STT/TTS at 400–750 ms, trading latency for real LLM capability. Sources: [kyutai-labs/moshi](https://github.com/kyutai-labs/moshi), [kyutai.org/unmute](https://kyutai.org/unmute/), [Moshi paper](https://arxiv.org/html/2410.00037v2).
- **Agent frameworks** — **LiveKit Agents** (WebRTC rooms + native SIP since 2025, infrastructure-first) and **Pipecat** (Python frame-pipeline, vendor-neutral, telephony via Twilio/Daily). Both are the standard glue when self-hosting the media path; both are Python-first, which cuts against this repo's Rust asset. Sources: [livekit/agents](https://github.com/livekit/agents), [WebRTC.ventures comparison](https://webrtc.ventures/2026/03/choosing-a-voice-ai-agent-production-framework/).
- **Telephony transport wisdom:** stay on **G.711 to avoid transcoding**, keep the media path short, send RTP keepalives during think-gaps; direct SIP beats WebSocket-bridged media for baseline latency; budget < 600 ms p95, ideal 200–500 ms. Sources: [Twilio latency guide](https://www.twilio.com/en-us/blog/developers/best-practices/guide-core-latency-ai-voice-agents), [ecosmob](https://www.ecosmob.com/blog/fix-voice-ai-latency-twilio-sip).

### 4.2 What PhoneCheck already gives us

| Asset | Reuse for a voice agent |
|---|---|
| SIP UAC: INVITE/ACK/BYE, digest auth, REGISTER, retransmit timers (RFC 3261 Timer A/B) | ~70% of a SIP endpoint; needs the UAS half (answering) |
| RTP receive: jitter buffer (reorder/loss/wraparound), G.711 µ-law/A-law decode tables | Inbound audio path done; needs an encode+send path (G.711 *encode* tables, 20 ms pacing) |
| Rubato resampling 8 k→16 k | Needs 8 k↔24 k both directions (Realtime API uses 24 kHz PCM16; it also accepts G.711 directly, which may remove resampling entirely) |
| STUN + CGNAT discovery, NAT hole punching, RTP keepalive | Directly reusable for a self-hosted media path |
| voip.ms account + credentials, business-hours scheduler (Pacific, DST-safe), Pushover alerting, health/metrics server | Ops scaffolding for the agent service |
| Whisper + embedding pipeline | Keep as the **watchdog** — point PhoneCheck at the agent |
| `/Users/roger/Dev/parakeet-rs` (local Parakeet TDT ASR, sherpa-onnx/CoreML) | Local ASR for a cascaded fallback / after-hours voicemail transcription |

Missing for any self-hosted duplex path: SIP UAS (parse INVITE, build 200 OK with SDP answer), RTP send (encode, sequence/timestamp pacing, SSRC), bidirectional bridge to the model, barge-in playout control.

### 4.3 Recommended architecture

**Route the phone line to OpenAI's SIP endpoint directly; write only the control plane in Rust.** voip.ms can forward a DID to an external SIP URI at no per-minute forwarding cost ([voip.ms wiki](https://wiki.voip.ms/article/SIP_URI)), and OpenAI terminates SIP natively. No self-hosted media path — the two hardest problems (duplex media transport, barge-in) are handled by the provider.

```
Caller ──PSTN──▶ voip.ms DID
                    │  (ring group: shop phones first during business hours,
                    │   agent on no-answer; agent immediately after hours)
                    ▼  SIP URI forward
        sip:proj_…@sip.api.openai.com;transport=tls   (G.711 end-to-end)
                    │
                    │ webhook: realtime.call.incoming
                    ▼
        ┌───────────────────────────────┐
        │  cubic-answer (Rust, axum)    │   ← new, small
        │  • accept(call_id, prompt,    │
        │    voice, tools)              │
        │  • WS monitor of transcript   │
        │  • tools: take_message →      │
        │    Pushover/email; hours/     │
        │    directions FAQ; transfer   │
        │  • refer(call_id, tel:+1…)    │  → human fallback (cell/desk)
        │  • logs full transcript       │
        └───────────────────────────────┘
                    ▲
        PhoneCheck (existing) keeps calling hourly and
        verifying the greeting — now it monitors the agent.
```

- **Latency budget (<500 ms mouth-to-ear):** carrier+voip.ms forward ~40–80 ms; OpenAI SIP edge + model first-audio ~250–350 ms (vendor-reported); return path ~40–60 ms → ~350–500 ms, inside target. No hop through our hardware, so our NAT/CGNAT problems vanish.
- **Barge-in:** server-side semantic VAD interrupts generation natively; nothing to build.
- **Duplex model placement:** today the duplex(-ish) model is gpt-realtime-2.1 at OpenAI's edge; when GPT-Live-1 gets API access, it replaces the model name in the `accept` call — same plumbing.
- **Human fallback:** business hours → voip.ms ring group rings the shop first, agent only on no-answer (agent = receptionist of last resort); any time the caller asks for a person or the model is unsure → `refer` to a cell. After hours → agent answers immediately, takes a structured message, pushes it via the existing Pushover integration.
- **Cost:** at ~$0.10–0.40/min (mini: ~a third), a shop taking even 20 agent-minutes/day is ≲ $50–250/month — small next to a missed machine-service call. (Approximate; secondary sources.)
- **Guardrails:** the agent prompt must contain no personal identity or internal operational details (public-facing surface); log all transcripts; explicit "I'm an automated assistant" disclosure; no pricing commitments — take a message instead.

**Alternative paths, and when to take them:**

- **Plan B — self-hosted media bridge in Rust (reuse phonecheck's stack):** extend to UAS + RTP send, bridge G.711 ↔ Realtime API WebSocket (it accepts G.711/PCMU frames directly, avoiding resampling). Choose this only if provider-SIP proves unreliable, or for provider independence (the same bridge can speak to Gemini Live / Nova 2 Sonic / a local model). This is where the existing jitter buffer, NAT traversal, and codec code pay off. Watch items from Section 3: fix the Timer-B-after-1xx behavior and the resampler flush first.
- **Plan C — local/open stack:** Unmute-style cascade (parakeet-rs ASR → local LLM → Kyutai streaming TTS) at 400–750 ms; realistic as an **outage fallback** (answer, apologize, take voicemail, transcribe locally with parakeet-rs) rather than the primary answerer. Moshi itself is the wrong tool: superb duplex latency, but a front-desk agent needs tools and factual discipline more than overlap talk.

### 4.4 Phased plan and effort

| Phase | Deliverable | Effort |
|---|---|---|
| **0 — Prototype** | Spare voip.ms DID → SIP-URI forward to OpenAI; `cubic-answer` webhook service (Rust/axum, ~300–500 LOC): accept call, Cubic system prompt, `take_message` tool → Pushover; transcript logging. Call it, iterate on the prompt. | **1–2 days** |
| **1 — Production on the real number** | Ring-group failover (shop first in business hours, using the scheduler's hours logic), immediate answer after hours; `refer` transfer to human; spam screening; disclosure line; PhoneCheck pointed at the agent as watchdog; runbook. | **~1 week** |
| **2 — Provider independence (optional)** | Rust media bridge: SIP UAS + RTP send + G.711 WebSocket bridge, pluggable backends (OpenAI / Gemini Live / Nova 2 Sonic). Only if Phase 1 shows provider-SIP pain or cost pressure. | **2–4 weeks** |
| **3 — Local fallback (optional)** | After-hours/outage voicemail path: answer locally, record, transcribe with parakeet-rs, Pushover the transcript. Cascaded local agent (Unmute-style) if desired. | **2–3 weeks** |

**Bottom line:** with OpenAI's SIP-native Realtime API and voip.ms SIP-URI forwarding, a working agent that answers Cubic's phone is a *days*-scale project, and PhoneCheck's most valuable role is the one it already has — being the thing that calls the agent every hour and screams if the greeting is wrong. The full self-hosted Rust media path is a good Phase-2 insurance policy, not the starting point.

---

## Appendix: verification log

- `cargo build --release`: clean (4 lib warnings, pre-existing).
- `cargo test --release`: 388 passed, 0 failed across all targets (lib 239, adversarial_config 19, adversarial_digest 24, adversarial_g711 28, adversarial_rtp 35, adversarial_sip_messages 35, integration 8).
- `cargo clippy --all-targets`: exit 0, 0 errors.
- Regenerated model checksums match `scripts/export_wav2vec2.py` pinned values (`wav2vec2_encoder.onnx`, `wav2vec2_encoder.onnx.data`: OK).
- New regression tests: 2 in `src/scheduler.rs`, 2 in `src/rtp/receiver.rs` — all pass.
- UNVERIFIED items are labeled inline (latency and pricing figures from vendor posts/secondary sources; OpenAI's own announcement pages block fetching).
