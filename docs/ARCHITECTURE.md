# Cue — Architecture

## Crate Structure

```
cue/
├── Cargo.toml              # workspace root
├── crates/
│   ├── cue-core/           # library: all business logic
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── audio/
│   │       │   ├── mod.rs
│   │       │   ├── pipewire.rs     # primary: direct PipeWire bindings
│   │       │   ├── cpal.rs         # fallback: ALSA/PulseAudio
│   │       │   ├── ring.rs         # rtrb ring buffers
│   │       │   └── vad.rs          # voice activity detection
│   │       ├── stt/
│   │       │   ├── mod.rs
│   │       │   ├── whisper.rs      # whisper.cpp via whisper-rs
│   │       │   └── candle.rs       # pure-Rust fallback (feature-gated)
│   │       ├── llm/
│   │       │   ├── mod.rs
│   │       │   ├── provider.rs     # LlmProvider trait
│   │       │   ├── anthropic.rs    # Anthropic SSE
│   │       │   ├── openai.rs       # OpenAI-compatible SSE
│   │       │   ├── ollama.rs       # Ollama NDJSON
│   │       │   └── router.rs       # failover router
│   │       ├── kb/
│   │       │   ├── mod.rs
│   │       │   ├── store.rs        # KnowledgeStore trait + SQLite impl
│   │       │   ├── embed.rs        # ONNX embedding (all-MiniLM-L6-v2)
│   │       │   ├── chunk.rs        # document chunking
│   │       │   └── ingest.rs       # PDF, Markdown, TXT parsers
│   │       ├── context.rs          # prompt assembly: sys + RAG + transcript + query
│   │       ├── session.rs          # SQLite session/transcript/exchange storage
│   │       ├── output/
│   │       │   ├── mod.rs          # OutputSink trait
│   │       │   ├── stdout.rs
│   │       │   ├── json.rs
│   │       │   ├── notify.rs
│   │       │   ├── tui.rs          # ratatui (feature-gated)
│   │       │   └── daemon.rs       # Unix socket
│   │       ├── hotkeys/
│   │       │   ├── mod.rs          # HotkeyListener trait + resolution order
│   │       │   ├── portal.rs       # ashpd XDG portal
│   │       │   ├── hyprland.rs     # Hyprland IPC
│   │       │   └── evdev.rs        # fallback (feature-gated)
│   │       └── config.rs           # TOML config + XDG dirs + env vars
│   │
│   └── cue-cli/            # binary: CLI entry point only
│       └── src/
│           ├── main.rs             # clap entry point
│           └── commands/
│               ├── start.rs        # cue start
│               ├── ask.rs          # cue ask
│               ├── listen.rs       # cue listen (STT only)
│               ├── load.rs         # cue load <file>
│               ├── kb.rs           # cue kb list/search/remove
│               ├── history.rs      # cue history list/search/export
│               ├── providers.rs    # cue providers list/add/test
│               ├── prompts.rs      # cue prompts list/use/add
│               ├── devices.rs      # cue devices
│               ├── models.rs       # cue models list/download
│               └── config.rs       # cue config / config edit
│
├── prompts/                # default system prompt templates (compiled via include_str!)
│   ├── coding.toml
│   ├── behavioral.toml
│   ├── system_design.toml
│   ├── meeting.toml
│   ├── sales.toml
│   └── general.toml
│
└── install.sh              # curl|sh installer
```

---

## Threading Model

```
┌─────────────────────────────────────────────────────────────────┐
│                         OS Threads                              │
│                                                                 │
│  ┌──────────────────┐  ┌──────────────────┐                    │
│  │ PipeWire Thread  │  │  Whisper Thread  │                    │
│  │                  │  │                  │                    │
│  │ pw_main_loop_run │  │ CPU-bound infer  │                    │
│  │ → process cb     │  │ (blocks OK here) │                    │
│  │ → ring.push()    │  │                  │                    │
│  └────────┬─────────┘  └────────▲─────────┘                    │
│           │                     │                               │
└───────────┼─────────────────────┼───────────────────────────────┘
            │ rtrb (lock-free)    │ tokio::mpsc (bounded, 16)
┌───────────▼─────────────────────┼───────────────────────────────┐
│                    Tokio Runtime │                               │
│                                 │                               │
│  ┌──────────────┐  ┌────────────┴──┐  ┌─────────────────────┐  │
│  │  VAD Task    │  │  STT Router   │  │  Context + LLM Task │  │
│  │              │  │               │  │                     │  │
│  │ reads ring   │  │ dispatches to │  │ RAG + prompt build  │  │
│  │ emits        │  │ whisper thread│  │ SSE stream          │  │
│  │ Utterance    │  │               │  │ output emit         │  │
│  └──────────────┘  └───────────────┘  └─────────────────────┘  │
│                                                                 │
│  ┌──────────────┐  ┌───────────────┐  ┌─────────────────────┐  │
│  │ Hotkey Task  │  │ Session Flush │  │   Output Task       │  │
│  │              │  │               │  │                     │  │
│  │ ashpd portal │  │ SQLite WAL    │  │ stdout/tui/notify   │  │
│  │ stream recv  │  │ every 5s      │  │ /daemon socket      │  │
│  └──────────────┘  └───────────────┘  └─────────────────────┘  │
└────────────────────────────────────────────────────────────────┘
```

**Key invariants:**
- PipeWire thread NEVER touches tokio
- Whisper thread NEVER touches tokio (spawned via `tokio::task::spawn_blocking`)
- All inter-thread communication: either `rtrb` (audio data) or `tokio::sync::mpsc` (control/data)
- SQLite: single writer via `tokio::sync::Mutex<Connection>`, WAL mode for reads

---

## Data Flow

```
[PipeWire]
    │
    │ f32 samples (16kHz mono)
    ▼
[rtrb ring buffer] ← lock-free, ~4MB
    │
    │ read by VAD task
    ▼
[VAD] ──energy gate──▶ skip (no speech)
    │
    │ Utterance { samples, duration_ms, channel }
    │ via tokio::mpsc (capacity: 16)
    ▼
[STT router]
    │
    │ spawn_blocking → whisper thread
    ▼
[whisper.cpp] → text: String
    │
    │ TranscriptEntry { text, channel, confidence, spoken_at }
    │ via tokio::mpsc
    ▼
[Context Engine]
    │
    │ Trigger? (hotkey / auto / manual)
    │ If triggered:
    │   1. retrieve top-k from KB (sqlite-vec cosine search)
    │   2. build prompt: system + KB context + transcript window + query
    ▼
[LLM Router]
    │
    │ SSE stream: Pin<Box<dyn Stream<Item=Result<String>>>>
    ▼
[Output Sink]
    │
    ├──▶ stdout (default)
    ├──▶ TUI (ratatui pane update)
    ├──▶ notify-send (desktop notification)
    └──▶ Unix socket (daemon clients)
    │
    ▼
[Session Store] — async write to SQLite (exchanges table)
```

---

## Key Design Decisions

### Why SQLite (not PostgreSQL, not Redis, not files)?

1. Single binary, zero dependencies. No postgres server, no redis server.
2. sqlite-vec extension provides vector similarity search in the same file.
3. WAL mode gives safe concurrent reads + single writer.
4. FTS5 extension provides full-text search over transcripts.
5. The entire knowledge base + session history is one portable `.db` file.

### Why rtrb (not std::sync::mpsc or tokio::mpsc)?

PipeWire's process callback is a real-time context. It cannot allocate memory, cannot block, cannot acquire mutexes. `rtrb` is a single-producer single-consumer ring buffer designed specifically for real-time audio. It's lock-free and allocation-free on the write path.

### Why spawn_blocking for whisper?

Whisper inference is CPU-bound and takes 100-500ms. Running it on tokio's async executor would block the entire runtime. `spawn_blocking` moves it to a dedicated OS thread pool, keeping the async runtime responsive.

### Why ashpd over evdev?

evdev requires:
1. User in `input` group (security permission escalation)
2. Polling input device files in a tight loop
3. No awareness of virtual keyboards or remapping

ashpd (XDG Desktop Portal) is the proper Wayland approach:
1. No special permissions needed
2. Compositor handles the binding and activation
3. Works across all portal-supporting compositors
4. Future-proof (this is the upstream standard)

### Why two crates (cue-core + cue-cli)?

`cue-core` is a library. This means:
1. It can be tested independently without CLI overhead
2. Integration tests can use the library directly
3. Future: other interfaces (daemon protocol, language bindings) without duplicating logic
4. Clear separation: business logic vs UI

---

## State Machine: Audio Capture

```
IDLE ──[start]──▶ RUNNING ──[stop]──▶ IDLE
                     │
                     ├──[speech detected]──▶ CAPTURING
                     │                          │
                     │                    [silence 1s]──▶ FLUSHING ──▶ RUNNING
                     │                          │
                     │                    [max 15s]──▶ FLUSHING ──▶ RUNNING
                     │
                     └──[hotkey: toggle]──▶ PAUSED ──[hotkey: toggle]──▶ RUNNING
```

---

## Error Handling Strategy

All errors propagate via `anyhow::Result`. Error types:

```rust
#[derive(thiserror::Error, Debug)]
pub enum CueError {
    #[error("Audio device not found: {0}")]
    AudioDevice(String),
    #[error("STT model not found at {0}")]
    ModelNotFound(PathBuf),
    #[error("All LLM providers failed: {0:?}")]
    AllProvidersFailed(Vec<String>),
    #[error("Knowledge base corrupted: {0}")]
    KbCorrupted(String),
    #[error("Config error: {0}")]
    Config(String),
}
```

**Failure modes and recovery:**

| Failure | Recovery |
|---|---|
| PipeWire disconnects | Retry with exponential backoff (max 5s), then fall back to cpal |
| Whisper thread panics | Restart thread, log error, continue listening |
| LLM provider timeout | Try next provider in failover chain |
| All LLM providers fail | Print error to stderr, continue transcribing |
| SQLite write fails | Log error, keep in-memory buffer, retry on next flush |

**The golden rule:** audio capture and transcription never stop due to LLM failures. The pipeline degrades gracefully.

---

## Security Boundaries

```
External input (audio) ──▶ VAD ──▶ whisper (local, sandboxed)
                                         │
                                         ▼
                                  transcript (trusted)
                                         │
                                         ▼
                                  context builder ──▶ LLM API (HTTPS, TLS 1.3)
                                                           │
                                                           ▼
                                                    response (untrusted)
                                                           │
                                                     sanitize ──▶ output
```

- API keys: stored in OS keyring (D-Bus Secret Service), never in config files
- Responses from LLM API are treated as untrusted text — no eval, no execution
- Markdown rendering uses pulldown-cmark (no JS execution surface)
- Config files validated at load time, no dynamic eval

---

## Observability

```rust
// Structured logging via tracing
tracing::info!(
    provider = %provider.name(),
    model = %model,
    tokens = tokens_used,
    latency_ms = latency.as_millis(),
    "LLM exchange complete"
);

// SOTTO_LOG=debug cue start  (maps to RUST_LOG)
```

Metrics (not shipped in v0.1, planned for v0.3):
- Audio buffer fill level
- VAD speech/silence ratio
- STT latency p50/p95
- LLM first-token latency
- Token usage per session
