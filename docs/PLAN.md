# Cue — Implementation Plan

## Project Goal

Ship a single Rust binary (`cue`) that:
1. Captures system audio + microphone via PipeWire
2. Transcribes locally via whisper.cpp
3. Retrieves context from a local knowledge base (your docs)
4. Queries an LLM with assembled context
5. Streams the response to your terminal

Target: Linux-first, Wayland-native, <5MB binary, <80MB RAM, <300ms STT latency.

---

## Phase 1: v0.1 — "Pipe Works" (Week 1-2)

**Goal:** End-to-end pipeline working. Audio in, text out.

### Milestone 1.1 — Project Scaffolding
- [ ] Initialize cargo workspace with two crates: `cue-core`, `cue-cli`
- [ ] Set up `Cargo.toml` with all workspace dependencies
- [ ] Add `config.rs`: load `~/.config/cue/cue.toml` + env vars + XDG dirs
- [ ] Add `tracing` setup with `RUST_LOG` env var control
- [ ] `cue --version` and `cue help` working

### Milestone 1.2 — Audio Capture
- [ ] PipeWire stream creation (`pipewire-rs`)
  - [ ] Connect as input with `STREAM_CAPTURE_SINK=true` (loopback capture)
  - [ ] Process callback: dequeue buffer → push to `rtrb` ring
  - [ ] PipeWire main loop on dedicated OS thread
- [ ] Monitor source discovery (traverse PipeWire graph, handle EasyEffects)
- [ ] Microphone capture (second PipeWire stream)
- [ ] cpal fallback (feature-gated, `--features cpal-fallback`)
- [ ] `cue devices` command: list all available audio sources/sinks

### Milestone 1.3 — Voice Activity Detection
- [ ] Tier 1: energy gate (RMS threshold, ~10 lines)
- [ ] Tier 2: Silero VAD via ONNX runtime (`ort` crate)
  - [ ] Download silero VAD model to `~/.local/share/cue/models/`
  - [ ] Run only on energy-gate candidates (avoid unnecessary inference)
- [ ] VAD state machine: SILENCE → SPEECH → SILENCE → emit Utterance
- [ ] Configurable: `vad_sensitivity`, `vad_silence_timeout_ms`

### Milestone 1.4 — Speech-to-Text
- [ ] `whisper-rs` integration
  - [ ] Load model from `~/.local/share/cue/models/ggml-tiny.bin`
  - [ ] Run inference via `tokio::task::spawn_blocking`
  - [ ] Emit `TranscriptEntry { text, channel, confidence, spoken_at }`
- [ ] `cue models download tiny` command
- [ ] Channel labels: `[SYS]` for system audio, `[MIC]` for microphone

### Milestone 1.5 — LLM Integration (Anthropic only)
- [ ] `LlmProvider` trait definition
- [ ] Anthropic implementation (SSE streaming)
  - [ ] API key from env var `ANTHROPIC_API_KEY` or `~/.config/cue/.env`
  - [ ] `Secret<String>` wrapping via `secrecy` crate
  - [ ] SSE parsing: `event: content_block_delta` format
- [ ] Basic prompt: system prompt (hardcoded general template) + last 5 transcript entries + query
- [ ] Interactive first-run setup: prompt for API key, store to keyring

### Milestone 1.6 — Trigger + Output
- [ ] Hotkey trigger (default: `Super+Return`)
  - [ ] ashpd portal integration (Hyprland/GNOME/KDE/Sway)
  - [ ] Fallback: read from stdin `!` prefix
- [ ] `StdoutOutput`: transcript → stderr, responses → stdout
- [ ] `cue start` command wiring everything together
- [ ] `cue ask "question"` — manual one-off query
- [ ] Ctrl+C graceful shutdown

### Milestone 1.7 — Config + Install
- [ ] Full TOML config schema with defaults
- [ ] `install.sh` script (detect arch, download binary, download tiny model)
- [ ] `cue config` prints current config
- [ ] README with quickstart

**v0.1 acceptance criteria:**
```bash
cue start
# → detects audio devices
# → loads whisper tiny model
# → listens to system audio
# → transcribes speech to stderr
# → on Super+Return: sends last 60s transcript to Claude
# → streams response to stdout
```

---

## Phase 2: v0.2 — "Actually Useful" (Week 3-4)

**Goal:** RAG working. Multi-provider. Daily useful.

### Milestone 2.1 — SQLite Foundation
- [ ] `rusqlite` + WAL mode setup
- [ ] Schema migration system (simple version table + migration files)
- [ ] Schema: `documents`, `chunks`, `chunk_vec` (sqlite-vec), `sessions`, `transcript`, `exchanges`
- [ ] `KnowledgeStore` trait definition

### Milestone 2.2 — Document Ingestion
- [ ] Text file ingestion (`.txt`, `.md`)
- [ ] PDF ingestion via `pdf-extract`
- [ ] Chunking: 512 tokens, 64 token overlap
- [ ] `cue load <file>` command
- [ ] Progress bar (indicatif) for large document sets

### Milestone 2.3 — Embeddings
- [ ] ONNX runtime integration (`ort` crate)
- [ ] Download `all-MiniLM-L6-v2.onnx` to `~/.local/share/cue/models/`
- [ ] Tokenize via `tokenizers` crate
- [ ] Embed via ONNX inference
- [ ] Store via `zerocopy::AsBytes` (zero-copy f32 → bytes for sqlite-vec)

### Milestone 2.4 — Vector Search
- [ ] sqlite-vec virtual table setup
- [ ] Cosine distance search: `vec_distance_cosine`
- [ ] `cue kb search "query"` command
- [ ] `cue kb list` / `cue kb remove`

### Milestone 2.5 — RAG Context Injection
- [ ] Context engine: assemble prompt
  - [ ] System prompt (active template)
  - [ ] Top-k KB chunks (embed query, search, inject)
  - [ ] Recent transcript (last 2 min, ~400 tokens)
  - [ ] Query text
- [ ] Token budget enforcement (~1500 total)
- [ ] Transcript window: configurable `transcript_window_secs`

### Milestone 2.6 — Multi-Provider
- [ ] OpenAI-compatible implementation (covers OpenAI, Groq, Mistral, OpenRouter)
- [ ] Ollama implementation (NDJSON streaming)
- [ ] Failover router: try primary → failover chain
- [ ] `cue providers list` / `cue providers add` / `cue providers test`
- [ ] `keyring` integration for all provider API keys

### Milestone 2.7 — System Prompts
- [ ] 6 built-in templates compiled via `include_str!`
- [ ] `cue prompts list` / `cue prompts use <name>`
- [ ] Custom prompts: `~/.config/cue/prompts/*.toml`
- [ ] `cue prompts add <file>` / `cue prompts edit <name>` (opens `$EDITOR`)

**v0.2 acceptance criteria:**
```bash
cue load resume.pdf
cue load jd.txt
cue prompt use behavioral
cue start
# → RAG context included in prompts
# → Answers grounded in loaded documents
# → Works with Ollama (local, no API key)
```

---

## Phase 3: v0.3 — "Daily Driver" (Week 5-7)

**Goal:** All output modes. Session history. Daemon mode. Hotkeys.

### Milestone 3.1 — TUI Mode
- [ ] `ratatui` + `crossterm` integration (feature-gated)
- [ ] Layout: transcript pane (left) + response pane (right) + status bar
- [ ] Streaming token display in response pane
- [ ] Keyboard shortcuts within TUI
- [ ] `cue start --tui`

### Milestone 3.2 — Session Store
- [ ] `SessionStore` implementation
- [ ] Auto-start session on `cue start`
- [ ] Auto-title from first 3 system-channel entries (send to LLM for title)
- [ ] Flush transcript to SQLite every 5s (crash recovery)
- [ ] `cue history list` / `cue history search` (FTS5) / `cue history export`

### Milestone 3.3 — Daemon Mode
- [ ] Unix socket at `/run/user/$UID/cue.sock`
- [ ] Protocol: simple JSON-newline (request/response + streaming events)
- [ ] `cue start --daemon` (forks? uses systemd user service?)
- [ ] `cue ask` communicates with daemon via socket
- [ ] `cue stop` sends shutdown signal via socket

### Milestone 3.4 — Notification Output
- [ ] `notify-rust` integration
- [ ] `cue start --notify` mode
- [ ] Response chunked into notification-sized pieces
- [ ] Optional: clipboard copy of full response

### Milestone 3.5 — Global Hotkeys (Full)
- [ ] ashpd portal: toggle recording, trigger LLM, cycle prompt
- [ ] Hyprland IPC fallback
- [ ] evdev fallback (feature-gated)
- [ ] Configurable key bindings in `cue.toml`

### Milestone 3.6 — Stealth + Camouflage
- [ ] Process name camouflage via `prctl`
- [ ] Configurable: `camouflage_name = "pipewire-pulse"`
- [ ] Documentation: WM config snippets for Hyprland, Sway, i3, Niri

### Milestone 3.7 — Export
- [ ] `cue history export <id> --format md` → Markdown
- [ ] `cue history export <id> --format json` → structured JSON
- [ ] Auto-export option in config

---

## Phase 4: v0.4 — "Ship It" (Week 8-10)

**Goal:** Distributable. Documented. Demoed.

### Milestone 4.1 — Static Builds
- [ ] musl target: `x86_64-unknown-linux-musl`
- [ ] Cross-compile: `aarch64-unknown-linux-musl`
- [ ] Bundle whisper.cpp statically (or document dynamic linking)
- [ ] Verify binary size <5MB (stripped)

### Milestone 4.2 — CI/CD
- [ ] GitHub Actions: build + test on push
- [ ] Release workflow: tag → build musl binaries → GitHub Release
- [ ] Auto-update `install.sh` with latest release URL

### Milestone 4.3 — Package Distribution
- [ ] `cargo install cue` (crates.io publish)
- [ ] Nix flake
- [ ] AUR PKGBUILD (`cue-bin`)

### Milestone 4.4 — Documentation
- [ ] mdBook setup
- [ ] Quickstart guide
- [ ] WM integration guide (Hyprland, Sway, i3, Niri)
- [ ] All commands documented
- [ ] Troubleshooting section (PipeWire, API keys, model download)

### Milestone 4.5 — Demo
- [ ] asciinema recording (60s: install → load docs → start → demo query)
- [ ] README with gif/asciinema embed
- [ ] Show HN post

---

## Phase 5: v0.5 — "The Brain" (Future)

- [ ] Speaker diarization (native-pyannote-rs + burn framework)
- [ ] Auto-prompt switching (keyword detection in transcript)
- [ ] Cross-session vector search (query historical transcripts)
- [ ] Optional `KnowledgeStore` backend swap (external vector DB)
- [ ] GPU acceleration for embeddings

---

## Implementation Order (Critical Path)

```
1. Config + XDG dirs              (30 min)
2. PipeWire audio capture         (1 day)
3. VAD (energy gate only first)   (2 hours)
4. Whisper STT                    (3 hours)
5. Anthropic LLM streaming        (3 hours)
6. Basic stdout output            (1 hour)
7. cue start wiring               (2 hours)
─────────────────────────────────────────
MVP working: ~3 days

8. SQLite schema + migrations     (2 hours)
9. Document chunking              (2 hours)
10. ONNX embeddings               (3 hours)
11. Vector search                 (2 hours)
12. RAG context injection         (3 hours)
13. Multi-provider + failover     (3 hours)
─────────────────────────────────────────
v0.2 complete: ~3 more days
```

---

## Risk Register

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| `pipewire-rs` API instability | Medium | High | Pin to exact version, test monitor source discovery on multiple setups |
| whisper.cpp build issues (C++ FFI) | Medium | High | Test musl build early (Week 1). Have candle fallback. |
| sqlite-vec API changes | Low | Medium | Pin to exact version |
| ashpd portal not available on target compositor | Medium | Low | evdev fallback implemented |
| ONNX model download fails (no internet) | Low | Low | Bundle tiny model in binary as option (--features bundled-model) |
| Binary size exceeds 5MB | Low | Low | LTO + strip + audit dependencies |
| RAG quality too low to be useful | Medium | High | Start with top-5 chunks, tune chunk_size, add BM25 hybrid search in v0.3 |

---

## Testing Strategy

### Unit Tests (cue-core)
- VAD state machine transitions
- Token budget enforcement in context engine
- Document chunking (edge cases: empty docs, very short docs)
- Config parsing (missing fields, invalid values)

### Integration Tests (cue-core)
- Audio → VAD → mock STT → context → mock LLM → output (full pipeline with mocks)
- SQLite: ingest → embed → search → verify top-k results
- Provider failover: primary fails → fallback succeeds

### Manual Testing Checklist (pre-release)
- [ ] `cue devices` shows correct PipeWire sources
- [ ] System audio captured (play YouTube, verify transcript)
- [ ] Microphone captured (speak, verify transcript)
- [ ] `cue load resume.pdf` → `cue kb search "experience"` returns relevant chunks
- [ ] RAG context in prompt (verify with `--debug` flag showing prompt)
- [ ] Anthropic streaming works
- [ ] Ollama works offline
- [ ] Failover: kill primary provider → falls back
- [ ] `cue start --daemon` + `cue ask "test"` + `cue stop`
- [ ] TUI mode renders correctly
- [ ] Notifications appear
- [ ] Hotkeys work (portal)
- [ ] Process shows as `pipewire-pulse` in htop
- [ ] Hyprland noscreencast rule works
- [ ] Static binary runs on clean VM (no extra deps)
- [ ] Memory: <80MB after 30 min session
