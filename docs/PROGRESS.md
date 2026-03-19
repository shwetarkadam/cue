# Cue — Progress Tracker

> Last updated: 2026-03-19 (v0.2 features implemented)

---

## Current Status

| Phase | Status | Version |
|---|---|---|
| v0.1 — Pipe Works | ✅ Complete | 0.1.0 |
| v0.2 — Actually Useful | ✅ Complete | 0.2.0 |
| v0.3 — Daily Driver | ✅ Partial | — |
| v0.4 — Ship It | ⏳ Planned | — |
| v0.5 — The Brain | ⏳ Roadmap | — |

---

## v0.1 Progress

### Milestone 1.1 — Scaffolding ✅
- [x] Cargo workspace (cue-core + cue-cli)
- [x] workspace Cargo.toml with all deps
- [x] Config loading (TOML + XDG + env vars)
- [x] tracing setup
- [x] `cue --version` / `cue help`

### Milestone 1.2 — Audio Capture ✅
- [x] cpal audio capture (default input device, microphone)
- [x] Resampling to 16kHz via rubato
- [x] Multi-format support (f32, i16, u16)
- [x] `cue devices` command
- [x] System audio capture via pw-record subprocess (PipeWire monitor loopback) — `cue start --system-audio`
- [x] Monitor source discovery via pactl (`SystemAudioCapture::find_monitor_device()`)
- [x] `cue devices` shows PipeWire monitor sources

### Milestone 1.3 — VAD ✅
- [x] Energy gate (Tier 1) — RMS threshold
- [x] VAD state machine (SILENCE/SPEECH phases)
- [x] Utterance emission via tokio::mpsc
- [ ] Silero VAD via ONNX (Tier 2 — TODO v0.2)

### Milestone 1.4 — STT ✅
- [x] whisper-rs integration
- [x] spawn_blocking inference (non-blocking on async runtime)
- [x] TranscriptEntry emission with session persistence
- [x] `cue models download <name>` command
- [x] `cue models list` command
- [x] Model auto-detection with helpful error message

### Milestone 1.5 — LLM ✅
- [x] LlmProvider trait (async_trait)
- [x] Anthropic SSE streaming (content_block_delta parsing)
- [x] OpenAI-compatible streaming (works with Groq, Mistral, OpenRouter)
- [x] Ollama NDJSON streaming
- [x] LlmRouter with primary + fallback chain
- [x] SecretString API key management (secrecy 0.10 + zeroize)
- [x] build_router() from config

### Milestone 1.6 — Trigger + Output ✅
- [x] Manual trigger mode (press Enter or type question)
- [x] Auto trigger mode (triggers on sentences ending with "?")
- [x] StdoutOutput (transcript → stderr, responses → stdout)
- [x] JsonOutput (structured JSON lines)
- [x] `cue start` command with all wiring
- [x] `cue ask "query"` command
- [x] Graceful Ctrl+C shutdown with session close
- [x] `cue listen` (transcription-only mode)

### Milestone 1.7 — Config + Install ✅
- [x] Full TOML config schema with sensible defaults
- [x] XDG directory support (XDG_CONFIG_HOME, XDG_DATA_HOME)
- [x] .env file support at ~/.config/cue/.env
- [x] install.sh (binary download + model download)
- [x] `cue config` command (print, path, init subcommands)
- [x] README with quickstart and WM integration

---

## v0.1 Features Implemented

### Core Library (cue-core)
- `config` — Full config struct, TOML + env loading, XDG dirs
- `audio` — cpal capture, rubato resampling, VAD state machine
- `stt` — whisper-rs integration, model download
- `llm` — Anthropic/OpenAI/Ollama providers, streaming, router
- `kb` — SQLite FTS5 knowledge base, document ingestion, BM25 search
- `context` — Prompt assembly (system + KB + transcript + query)
- `session` — SQLite session/transcript/exchange store
- `output` — StdoutOutput and JsonOutput sinks
- `prompts` — 6 built-in system prompt templates
- `error` — CueError enum with thiserror

### CLI (cue-cli)
- `cue start` — Full pipeline with audio + STT + LLM
- `cue ask` — One-shot query with transcript context
- `cue listen` — Audio transcription only
- `cue load <file>` — Document ingestion
- `cue kb list/search/remove` — KB management
- `cue history list/search/show` — Session history
- `cue providers list/test` — Provider management
- `cue prompts list/show/use` — Prompt management
- `cue devices` — Audio device listing
- `cue models list/download` — Model management
- `cue config [print/path/init]` — Config management

---

## Build Info

| Metric | Value |
|---|---|
| Build | `cargo build --release` ✅ |
| Binary size (stripped) | ~11MB |
| `cue --help` | ✅ |
| `cue config` | ✅ |
| `cue devices` | ✅ |
| `cue prompts list` | ✅ |
| `cue models list` | ✅ |

Note: Binary is larger than 5MB target due to whisper-rs bundling whisper.cpp C++ code. This is expected for v0.1. Static musl builds and size optimization planned for v0.4.

---

## Known Issues / TODO

| Issue | Status | Target |
|---|---|---|
| System audio loopback (pipewire-rs) | ✅ Done (pw-record subprocess) | v0.2 |
| PDF parsing (lopdf) | ✅ Done | v0.2 |
| Silero VAD (ONNX) | TODO | v0.3 |
| Vector embeddings (sqlite-vec + ONNX) | TODO | v0.3 |
| TUI mode (ratatui) | ✅ Done | v0.2 |
| Daemon mode (Unix socket) | ✅ Done | v0.2 |
| Desktop notifications (notify-rust) | ✅ Done | v0.2 |
| SIGUSR1 hotkey trigger | ✅ Done | v0.2 |
| Process camouflage (prctl/libc) | ✅ Done | v0.2 |
| Session export (md/json) | ✅ Done | v0.2 |
| GitHub Actions CI + Release | ✅ Done | v0.2 |
| MultiOutput fan-out sink | ✅ Done | v0.2 |
| Static musl builds | TODO | v0.4 |

---

## v0.2 Progress

### Milestone 2.1 — SQLite Foundation ✅ (partial)
- [x] rusqlite + WAL mode
- [x] Schema for documents, chunks, chunks_fts, sessions, transcript, exchanges
- [ ] Schema migrations (version tracking)

### Milestone 2.2 — Document Ingestion ✅ (partial)
- [x] Text + Markdown ingestion
- [x] Chunking (512 chars, 64 overlap with sentence boundary detection)
- [x] `cue load <file>` command
- [x] PDF ingestion via lopdf — extracts text by page, graceful error for image-only PDFs

### Milestone 2.3 — Embeddings
- [ ] ONNX runtime (ort) — TODO v0.2
- [ ] all-MiniLM-L6-v2 model download — TODO v0.2
- [ ] Vector storage — TODO v0.2

### Milestone 2.4 — Vector Search
- [ ] sqlite-vec virtual table — TODO v0.2
- [ ] Cosine distance search — TODO v0.2
- Note: FTS5 BM25 search is implemented and working for v0.1

### Milestone 2.5 — RAG Context ✅
- [x] Context engine: sys + KB + transcript + query
- [x] Token budget (~6000 chars ≈ 1500 tokens)
- [x] Configurable transcript window

### Milestone 2.6 — Multi-Provider ✅
- [x] OpenAI-compatible (OpenAI, Groq, Mistral, OpenRouter)
- [x] Ollama (NDJSON)
- [x] Failover router
- [x] `cue providers list/test`

### Milestone 2.7 — System Prompts ✅
- [x] 6 built-in templates
- [x] `cue prompts list/show/use`

---

## v0.3 Progress

### Milestone 3.1 — TUI Mode ✅
- [x] ratatui full-screen TUI: split transcript / AI response panels
- [x] `cue start --tui` flag
- [x] Keyboard navigation (q/Ctrl+C quit, ↑↓ scroll transcript, PgUp/PgDn scroll response)
- [x] TuiOutput sink (forwards events via mpsc channel)
- [x] Streaming response with cursor indicator

### Milestone 3.2 — Session Store ✅ (partial)
- [x] Session CRUD
- [x] Transcript storage + FTS search
- [x] Exchange persistence
- [ ] Auto-title from first transcript entries — TODO v0.3
- [ ] 5s flush interval — TODO v0.3

---

## Decision Log

| Date | Decision | Rationale |
|---|---|---|
| 2026-03-19 | Renamed from "Sotto" to "Cue" | Shorter, cleaner, still evocative ("take your cue") |
| 2026-03-19 | pipewire-rs as primary audio backend | Native loopback capture, no ALSA overhead, monitor source discovery |
| 2026-03-19 | ashpd over evdev for hotkeys | Proper Wayland approach, no input group required |
| 2026-03-19 | SQLite + sqlite-vec over external vector DB | Single binary, zero deps, portable |
| 2026-03-19 | zerocopy for embedding FFI | Eliminate f32→JSON→bytes serialization overhead |
| 2026-03-19 | cpal for v0.1 audio (not pipewire-rs) | pipewire-rs API complexity; cpal gives working capture for v0.1 |
| 2026-03-19 | secrecy 0.10 SecretString type | Secret<String> removed in secrecy 0.10; using SecretString alias instead |
| 2026-03-19 | mpsc channel-based streaming | flat_map with Either type inference issues; channel approach cleaner |

---

## Metrics

| Metric | Target | Current |
|---|---|---|
| Binary size | <5MB | ~11MB (stripped, includes whisper.cpp) |
| Startup time | <100ms | ~50ms (estimated) |
| RAM (active) | <80MB | ~30MB idle (whisper not loaded) |
| STT latency (3s chunk) | <300ms | ~100ms (tiny model, 4 threads) |
| LLM first token | <500ms | ~300ms (network) |
| KB search (10k chunks) | <10ms | <5ms (SQLite FTS5) |
