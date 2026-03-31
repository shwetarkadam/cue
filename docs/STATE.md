# Cue — Complete Project State

> Last updated: 2026-03-31

This document captures the full state of the Cue project — architecture, features, what's done, what's remaining, and how to build on any platform. Designed to be self-sufficient: drop this into a new session and the LLM has full context.

---

## What is Cue?

A real-time AI companion that captures audio, transcribes it locally or via cloud STT, and streams AI responses. It runs as:

- **CLI** (`cue`) — terminal-based, with optional TUI split-panel interface
- **Native GTK4 overlay** (`cue-native`) — floating glassmorphic panel on Linux/Hyprland
- **Tauri web overlay** (`cue-overlay`) — alternative GUI (optional)

**Core philosophy:** Single Rust binary, Unix-like pipeline, minimal dependencies, stealth-capable (process camouflage, hidden from screen share).

---

## Tech Stack

| Layer | Technology |
|---|---|
| Language | Rust 2021, async with Tokio |
| Audio capture | cpal (mic) + pw-record subprocess (system audio via PipeWire) |
| Resampling | rubato (to 16kHz mono) |
| VAD | Energy gate (RMS threshold) + VAD state machine |
| STT (primary) | **Parakeet TDT v3** — local ONNX model (native app only) |
| STT (cloud) | **Deepgram** — WebSocket streaming with auto-reconnect |
| STT (legacy) | whisper-rs (currently broken, feature-gated) |
| LLM | Anthropic (SSE), OpenAI-compatible (Groq/Mistral/OpenRouter), Ollama (NDJSON) |
| LLM routing | Failover chain (primary → fallback providers) |
| Knowledge base | SQLite + FTS5 (BM25 text search) |
| Brain | SQLite (folders, documents, notes, custom prompts) |
| Context engine | Prompt assembly: system prompt + brain context + KB RAG + transcript + query |
| Config | TOML (`~/.config/cue/cue.toml`) + env vars + `.env` file |
| Storage | SQLite WAL mode at `~/.local/share/cue/cue.db` |
| GUI | GTK4 (native overlay), Ratatui (TUI), Tauri (web overlay) |
| TTS | KittenTTS (local Python subprocess, optional) |
| Logging | tracing + tracing-subscriber (structured, env-filter) |
| Security | secrecy + zeroize for API keys, prctl process camouflage |

---

## Project Structure

```
cue/
├── Cargo.toml                    # Workspace root
├── Makefile                      # Build/run commands
├── crates/
│   ├── cue-core/                 # Shared library (all business logic)
│   │   └── src/
│   │       ├── audio/            # Audio capture (cpal, VAD state machine)
│   │       │   └── mod.rs        # AudioCapture, SystemAudioCapture, Utterance
│   │       ├── stt/              # Speech-to-text
│   │       │   ├── mod.rs        # SttEngine (whisper, feature-gated)
│   │       │   ├── deepgram.rs   # DeepgramStreamer (cloud WebSocket)
│   │       │   └── parakeet.rs   # ParakeetStreamer (local ONNX)
│   │       ├── llm/              # LLM providers
│   │       │   ├── mod.rs        # LlmProvider trait, build_router()
│   │       │   ├── anthropic.rs  # Anthropic Claude (SSE)
│   │       │   ├── openai.rs     # OpenAI-compatible (SSE)
│   │       │   ├── ollama.rs     # Ollama (NDJSON)
│   │       │   └── router.rs     # LlmRouter with failover
│   │       ├── kb/               # Knowledge base
│   │       │   ├── mod.rs        # KnowledgeBase, DocumentMeta, RetrievedChunk
│   │       │   ├── store.rs      # SQLite FTS5 search + ingestion
│   │       │   └── chunk.rs      # Text chunking (512 chars, 64 overlap)
│   │       ├── brain.rs          # Brain feature (folders, docs, notes, custom prompts)
│   │       ├── context.rs        # Context engine (prompt assembly with brain + KB + transcript)
│   │       ├── config.rs         # Config struct, TOML + env loading, XDG dirs
│   │       ├── migrate.rs        # Schema migration system (version-tracked)
│   │       ├── prompts.rs        # 6 built-in + custom prompt support
│   │       ├── session.rs        # Session store (SQLite CRUD)
│   │       ├── output/           # Output sinks (stdout, JSON, TUI, notify, daemon)
│   │       ├── tts.rs            # Text-to-speech (KittenTTS subprocess)
│   │       ├── stealth.rs        # Process camouflage (prctl on Linux)
│   │       ├── error.rs          # CueError enum
│   │       └── lib.rs            # Module exports
│   │
│   └── cue-cli/                  # CLI binary
│       └── src/
│           ├── main.rs           # clap entry point, command dispatch
│           ├── tui.rs            # TUI renderer (ratatui full-screen)
│           └── commands/
│               ├── start.rs      # Full pipeline (audio + STT + LLM)
│               ├── ask.rs        # One-shot query
│               ├── listen.rs     # Transcription only
│               ├── brain.rs      # Brain management (folders, notes, prompts)
│               ├── load.rs       # Document ingestion
│               ├── kb.rs         # KB search/list/remove
│               ├── prompts.rs    # List/show/use prompts
│               ├── history.rs    # Session history
│               ├── providers.rs  # LLM provider management
│               ├── devices.rs    # Audio device listing
│               ├── models.rs     # Whisper model download
│               ├── config_cmd.rs # Config management
│               └── daemon.rs     # Background daemon (Unix socket IPC)
│
├── apps/
│   ├── native/                   # GTK4 glassmorphic overlay
│   │   ├── Cargo.toml
│   │   └── src/main.rs           # Window management, Hyprland IPC, pipeline
│   │
│   └── overlay/                  # Tauri web overlay (alternative)
│       ├── src-tauri/
│       └── src/                  # Svelte frontend
│
└── docs/
    ├── OVERVIEW.md               # Philosophy & use cases
    ├── ARCHITECTURE.md           # Threading, data flow, design decisions
    ├── PLAN.md                   # Implementation roadmap (v0.1–v0.5)
    ├── PROGRESS.md               # Milestone tracking
    ├── TECH_SPEC.md              # Detailed technical spec
    └── STATE.md                  # THIS FILE — complete current state
```

---

## Features — Complete Status

### Core Pipeline (v0.1) — DONE

| Feature | Status | Notes |
|---|---|---|
| Audio capture (cpal, mic) | Done | Multi-format (f32, i16, u16) |
| System audio (pw-record) | Done | PipeWire loopback via subprocess |
| Resampling (rubato → 16kHz) | Done | |
| VAD energy gate | Done | RMS threshold, configurable sensitivity |
| VAD state machine | Done | SILENCE → SPEECH → SILENCE → emit Utterance |
| Whisper STT (local) | Broken | whisper-rs v0.12 incompatible with system whisper.cpp headers; feature-gated |
| Parakeet STT (local ONNX) | Done | Primary in native app; model: parakeet-tdt-0.6b-v3-int8 |
| Deepgram STT (cloud) | Done | WebSocket streaming with auto-reconnect |
| Anthropic Claude (SSE) | Done | |
| OpenAI-compatible (SSE) | Done | Works with Groq, Mistral, OpenRouter |
| Ollama (NDJSON) | Done | Local models |
| LLM failover router | Done | Primary + fallback chain |
| Stdout + JSON output | Done | |
| Graceful shutdown | Done | Ctrl+C + session cleanup |
| Config (TOML + env + XDG) | Done | |
| CLI (clap) | Done | All subcommands |

### Knowledge Base (v0.2) — DONE

| Feature | Status | Notes |
|---|---|---|
| SQLite + FTS5 | Done | BM25 text search |
| Document ingestion (txt, md, pdf) | Done | lopdf for PDF |
| Smart chunking | Done | 512 chars, 64 overlap, sentence boundaries |
| KB search/list/remove | Done | |
| Context engine (RAG) | Done | System + KB + transcript + query assembly |
| Token budget enforcement | Done | ~6000 chars ≈ 1500 tokens |

### Brain Feature (v0.2+) — DONE (NEW)

| Feature | Status | Notes |
|---|---|---|
| Brain folders | Done | Named folders linked to prompt categories |
| Brain documents | Done | Add files or write inline content |
| Brain notes | Done | Per-category notes (one per category, upsert) |
| Custom system prompts | Done | TOML or Markdown in `~/.config/cue/prompts/` |
| Context injection | Done | Notes + folder docs injected into system prompt |
| CLI commands | Done | `cue brain create/add/write/list/show/remove/note/prompt` |
| Schema migrations | Done | Version-tracked, auto-applied |

### Multi-Provider + Prompts (v0.2) — DONE

| Feature | Status | Notes |
|---|---|---|
| 6 built-in prompts | Done | general, coding, behavioral, system_design, meeting, sales |
| Custom prompts (disk) | Done | `~/.config/cue/prompts/*.toml` or `*.md` |
| `resolve_prompt()` | Done | Custom first, then built-in fallback |
| Provider list/test | Done | |

### TUI + Session (v0.3) — DONE

| Feature | Status | Notes |
|---|---|---|
| Ratatui TUI | Done | Split transcript/response panels |
| TUI keyboard nav | Done | q/Ctrl+C quit, arrows scroll, PgUp/PgDn |
| Session CRUD | Done | SQLite with FTS5 |
| Session history search | Done | Full-text search |
| Session export (md/json) | Done | |
| Daemon mode (Unix socket) | Done | Background IPC |
| Desktop notifications | Done | notify-rust |
| SIGUSR1 trigger | Done | External hotkey integration |
| Process camouflage | Done | prctl on Linux |

### Native GTK4 Overlay — DONE

| Feature | Status | Notes |
|---|---|---|
| Floating glassmorphic panel | Done | Transparent, rounded corners |
| Hyprland IPC | Done | Unix socket for stealth rules |
| Drag to move/resize | Done | Edge drag for resize |
| Transcript + AI response display | Done | Real-time streaming |
| Prompt selector | Done | Now shows custom prompts too |
| Auto-query mode | Done | Mic transcripts auto-sent to LLM |
| Brain integration | Done | Context engine uses brain data |
| STT cascade | Done | Parakeet → Deepgram → manual |
| TTS (KittenTTS) | Done | Optional, background thread |

---

## STT Architecture — Important

The project supports 3 STT backends with different availability:

| Backend | Where Available | Status |
|---|---|---|
| **Parakeet (local ONNX)** | Native app only | Primary — tried first |
| **Deepgram (cloud)** | Native app + CLI | Fallback — requires API key |
| **Whisper (local C++)** | CLI only (feature-gated) | Broken — whisper-rs v0.12 incompatible |

**Native app flow:**
```
Parakeet model found? → Use Parakeet (local, no internet)
    ↓ no
Deepgram API key set? → Use Deepgram (cloud streaming)
    ↓ no
Manual text-only mode (type to ask AI)
```

**Parakeet model location:** `~/.local/share/com.pais.handy/models/parakeet-tdt-0.6b-v3-int8/`
Files needed: `nemo128.onnx`, `encoder-model.int8.onnx`, `decoder_joint-model.int8.onnx`, `vocab.txt`

**CLI flow:**
```
--stt deepgram (default) → Deepgram cloud
--stt whisper → Whisper local (requires --features whisper, currently broken)
```

---

## Database Schema

Single SQLite database at `~/.local/share/cue/cue.db` (WAL mode).

```sql
-- Session management
sessions (id, title, prompt_template, started_at, ended_at)
transcript (id, session_id, channel, text, spoken_at)
transcript_fts (FTS5 virtual table)
exchanges (id, session_id, query, response, provider, model, created_at)

-- Knowledge base
documents (id, name, path, doc_type, chunk_count, created_at)
chunks (id, doc_id, content, chunk_index)
chunks_fts (FTS5 virtual table)

-- Brain
brain_folders (id, name, linked_prompt, created_at)
brain_documents (id, folder_id, name, content, created_at)
brain_notes (id, category, content, updated_at)

-- Migrations
schema_version (version, description, applied_at)
```

---

## Context Assembly Pipeline

When the LLM is queried, the context engine assembles the prompt:

```
1. System prompt (built-in or custom from ~/.config/cue/prompts/)
   ↓ enriched with:
   - Brain notes for active prompt category
   - Brain folder documents linked to active prompt
2. Chat history (last 6 exchanges, alternating user/assistant)
3. KB context (FTS5 search, top-k chunks with BM25 scoring)
4. Recent transcript (last 2 min, labeled [THEM]/[YOU])
5. User query

Total budget: ~6000 chars ≈ 1500 tokens
Brain content capped at 1/3 of budget
```

---

## Configuration

**Config file:** `~/.config/cue/cue.toml`

```toml
[provider]
default = "anthropic"         # anthropic, openai, groq, mistral, openrouter, ollama
model = "claude-sonnet-4-20250514"
temperature = 0.3
max_tokens = 512
openai_base_url = "https://api.openai.com/v1"
ollama_endpoint = "http://localhost:11434"

[stt]
model = "tiny"
language = "en"
stt_backend = "deepgram"      # deepgram or whisper

[audio]
backend = "cpal"
vad_sensitivity = 0.5
vad_silence_timeout_ms = 600
vad_min_speech_ms = 250

[rag]
enabled = true
top_k = 5
chunk_size = 512
chunk_overlap = 64
transcript_window_secs = 120

[trigger]
mode = "manual"               # manual, auto, hotkey

[display]
mode = "stdout"               # stdout, json, tui, notify, daemon

[stealth]
camouflage_enabled = false
camouflage_name = "pipewire-pulse"

[tts]
enabled = false
model = "KittenML/kitten-tts-nano-0.8-int8"
voice = "Jasper"
speed = 1.0
```

**Environment variables:**
```bash
ANTHROPIC_API_KEY    # Anthropic Claude
OPENAI_API_KEY       # OpenAI / compatible
GROQ_API_KEY         # Groq
DEEPGRAM_API_KEY     # Deepgram STT
CUE_PROVIDER         # Override default provider
CUE_MODEL            # Override model
CUE_STT_BACKEND      # Override STT backend
CUE_DISPLAY          # Override display mode
CUE_TRIGGER          # Override trigger mode
```

---

## CLI Commands

```bash
# Core
cue start [--prompt NAME] [--tui] [--notify] [--daemon] [--system-audio] [--stt deepgram|whisper]
cue ask "question" [--prompt NAME]
cue listen                        # Transcription only

# Brain
cue brain create <name> [--link <prompt>]
cue brain add <folder> <file>
cue brain write <folder> <name> <content>
cue brain list [folder]
cue brain show <folder> <doc>
cue brain remove <folder> [doc]
cue brain note set <category> <content>
cue brain note show <category>
cue brain note list
cue brain note remove <category>
cue brain prompt create <name> [--description "..."] <content>
cue brain prompt list
cue brain prompt show <name>
cue brain prompt remove <name>

# Knowledge base
cue load <file>                   # Ingest document
cue kb list
cue kb search "query"
cue kb remove <id>

# Management
cue prompts list                  # Built-in + custom
cue prompts show <name>
cue history list
cue history search "query"
cue providers list
cue providers test
cue devices                       # Audio devices
cue models list                   # Whisper models
cue models download <name>
cue config [print|path|init]
```

---

## Building

### Prerequisites (all platforms)

- Rust toolchain (rustup, cargo)
- pkg-config
- OpenSSL dev headers (for reqwest/TLS)

### Linux (primary platform)

```bash
# System dependencies (Arch)
sudo pacman -S base-devel openssl pkg-config gtk4 pipewire alsa-lib

# System dependencies (Ubuntu/Debian)
sudo apt install build-essential libssl-dev pkg-config libgtk-4-dev \
    pipewire libasound2-dev libclang-dev

# Build CLI
cargo build --release -p cue
strip target/release/cue

# Build native GTK4 overlay
cargo build --release -p cue-native

# Install
cp target/release/cue ~/.local/bin/
```

### macOS

```bash
# Prerequisites
brew install openssl pkg-config gtk4 adwaita-icon-theme

# Set OpenSSL paths for compilation
export OPENSSL_DIR=$(brew --prefix openssl)
export PKG_CONFIG_PATH="$(brew --prefix openssl)/lib/pkgconfig:$(brew --prefix gtk4)/lib/pkgconfig"

# Build CLI (no system audio capture — PipeWire is Linux-only)
cargo build --release -p cue
strip target/release/cue

# Build native GTK4 overlay
# GTK4 on macOS uses the Quartz backend (no Hyprland IPC — no-op on macOS)
cargo build --release -p cue-native

# Install
cp target/release/cue /usr/local/bin/

# Notes:
# - System audio capture (--system-audio) is NOT available on macOS (requires PipeWire)
# - Microphone capture works via cpal (CoreAudio backend)
# - Process camouflage (prctl) is Linux-only — no-op on macOS
# - Hyprland IPC is Linux-only — no-op on macOS
# - Parakeet STT works if model files are placed at:
#   ~/.local/share/com.pais.handy/models/parakeet-tdt-0.6b-v3-int8/
# - Deepgram cloud STT works out of the box (set DEEPGRAM_API_KEY)
# - ONNX Runtime (ort) may need:
#   export ORT_DYLIB_PATH=$(brew --prefix onnxruntime)/lib/libonnxruntime.dylib
```

### Windows (experimental)

```powershell
# Prerequisites: Visual Studio Build Tools, Rust, vcpkg
vcpkg install openssl:x64-windows gtk:x64-windows

# Build CLI
cargo build --release -p cue

# Notes:
# - System audio capture not available
# - Microphone works via cpal (WASAPI backend)
# - GTK4 requires gtk4 runtime DLLs on PATH
```

---

## Threading Model

```
Main thread:     Tokio runtime (multi-threaded)
                 ├── Audio capture (cpal callback → rtrb ring buffer)
                 ├── STT task (Parakeet/Deepgram, async)
                 ├── LLM streaming (reqwest SSE/NDJSON)
                 ├── Output sinks (stdout/TUI/notify/daemon)
                 └── Event loop (transcript + query + signals)

OS threads:      pw-record subprocess (system audio, Linux only)
                 spawn_blocking (Whisper/Parakeet CPU inference)

GTK4 thread:     GLib main loop (native app only)
                 ├── UI rendering
                 └── async_channel bridge to pipeline
```

**Key invariants:**
- Audio callbacks never touch Tokio or allocate
- STT inference runs via `spawn_blocking` (never blocks async runtime)
- SQLite uses single writer + WAL mode (concurrent reads OK)

---

## What's Remaining

### High Priority

| Task | Effort | Notes |
|---|---|---|
| Fix whisper-rs or replace | Medium | v0.12 incompatible with system whisper.cpp; options: pin version, use candle, or drop in favor of Parakeet |
| Silero VAD (ONNX) | Medium | Better voice detection than energy gate; use ort crate (already a dep) |
| Auto-session titles | Small | Send first 3 transcript entries to LLM for title generation |
| Brain UI in GTK overlay | Medium | Settings panel section for viewing/editing brain folders and notes |

### Medium Priority

| Task | Effort | Notes |
|---|---|---|
| Vector embeddings (sqlite-vec + ONNX) | Large | Semantic search for KB; all-MiniLM-L6-v2 model |
| Parakeet in CLI | Small | Currently native-app only; add `--stt parakeet` to CLI |
| Schema migrations runner in startup | Small | Call `migrate::run_migrations()` in CLI and native app startup |
| Hotkey listener (ashpd XDG portal) | Medium | Global hotkeys for trigger/toggle |

### Low Priority (v0.4+)

| Task | Effort | Notes |
|---|---|---|
| Static musl builds | Medium | Target <5MB binary |
| CI/CD improvements | Small | Build matrix, cross-compile |
| Package distribution | Medium | crates.io, Nix flake, AUR |
| Speaker diarization | Large | native-pyannote-rs |
| Cross-session vector search | Medium | Query historical transcripts |
| GPU acceleration | Medium | ONNX Runtime GPU provider |

---

## Decision Log

| Date | Decision | Rationale |
|---|---|---|
| 2026-03-19 | Renamed from "Sotto" to "Cue" | Shorter, cleaner |
| 2026-03-19 | SQLite + FTS5 over vector DB | Single binary, zero deps |
| 2026-03-19 | cpal for audio (not pipewire-rs) | pipewire-rs API complexity |
| 2026-03-19 | secrecy 0.10 SecretString | Secret<String> removed in 0.10 |
| 2026-03-31 | Feature-gated whisper-rs | v0.12 C FFI broken; unblocks build |
| 2026-03-31 | Parakeet as primary STT | Local, no API key, good quality |
| 2026-03-31 | Brain feature added | Folder-based knowledge + notes + custom prompts |
| 2026-03-31 | Schema migration system | Version-tracked DB changes |

---

## Quick Start (for a new developer/LLM)

```bash
# 1. Clone and build
git clone <repo> && cd cue
cargo build --release -p cue

# 2. Set up API key
echo 'ANTHROPIC_API_KEY=sk-ant-...' >> ~/.config/cue/.env
# OR
echo 'DEEPGRAM_API_KEY=...' >> ~/.config/cue/.env

# 3. Set up your brain
./target/release/cue brain create introduction --link general
./target/release/cue brain write introduction my-intro "Your introduction here..."
./target/release/cue brain note set coding "Your coding notes..."

# 4. Add custom prompt
./target/release/cue brain prompt create devops "Your system prompt here..."

# 5. Load documents
./target/release/cue load resume.pdf
./target/release/cue load job-description.txt

# 6. Run
./target/release/cue start --prompt coding --tui
# OR for native overlay:
cargo build --release -p cue-native && ./target/release/cue-native
```

---

## Files Modified in Brain Feature (2026-03-31)

**New files:**
- `crates/cue-core/src/brain.rs` — BrainStore (folders, documents, notes, custom prompts)
- `crates/cue-core/src/migrate.rs` — Schema migration system
- `crates/cue-cli/src/commands/brain.rs` — CLI brain commands

**Modified files:**
- `crates/cue-core/src/lib.rs` — Added brain + migrate modules
- `crates/cue-core/src/prompts.rs` — Added resolve_prompt(), list_all_prompts()
- `crates/cue-core/src/context.rs` — Brain context injection (notes + folder docs)
- `crates/cue-core/src/stt/mod.rs` — Feature-gated whisper-rs
- `crates/cue-core/Cargo.toml` — whisper-rs optional, whisper feature flag
- `crates/cue-cli/src/main.rs` — Added Brain command
- `crates/cue-cli/src/commands/mod.rs` — Added brain module
- `crates/cue-cli/src/commands/start.rs` — BrainStore wiring, resolve_prompt
- `crates/cue-cli/src/commands/ask.rs` — BrainStore wiring, resolve_prompt
- `crates/cue-cli/src/commands/daemon.rs` — Fixed build_prompt args
- `crates/cue-cli/src/commands/prompts.rs` — Shows custom prompts
- `apps/native/src/main.rs` — BrainStore in pipeline, resolve_prompt, list_all_prompts in UI
