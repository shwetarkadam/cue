# Cue — Technical Specification v3

> A single Rust binary. No app. No GUI framework. Pipe audio → transcribe → think → respond.
>
> **v3 changelog**: Wayland stealth analysis corrected (no universal exclusion API), `pipewire-rs` direct replaces `cpal` as primary Linux audio, `ashpd` replaces `evdev` for global hotkeys, speaker diarization added to roadmap, `zerocopy` for embedding FFI, expanded "Why Not" appendix with Tauri Wayland evidence.

---

## 1. System Architecture

```
                          cue (single process)
┌────────────────────────────────────────────────────────────────┐
│                                                                │
│  ┌─────────────┐    ┌─────────────┐    ┌──────────────────┐   │
│  │   Audio      │    │     STT     │    │    Context       │   │
│  │   Capture    │───▶│   Engine    │───▶│    Engine        │   │
│  │             │    │             │    │                  │   │
│  │ pipewire-rs │    │ whisper-rs  │    │ transcript buf   │   │
│  │ + cpal fbk  │    │ VAD gate    │    │ + RAG retrieval  │   │
│  │ dual channel│    │ chunked     │    │ + prompt build   │   │
│  └─────────────┘    └─────────────┘    └────────┬─────────┘   │
│                                                  │             │
│                                        ┌─────────▼─────────┐  │
│  ┌─────────────┐                       │    LLM Router     │  │
│  │  Knowledge  │◀─────────────────────▶│                   │  │
│  │  Base       │   retrieve chunks     │ provider trait    │  │
│  │             │                       │ SSE streaming     │  │
│  │ SQLite +    │                       │ failover chain    │  │
│  │ sqlite-vec  │                       └─────────┬─────────┘  │
│  │ ONNX embed  │                                 │             │
│  └─────────────┘                       ┌─────────▼─────────┐  │
│                                        │    Output         │  │
│  ┌─────────────┐                       │                   │  │
│  │  Session    │◀──────────────────────│ stdout (default)  │  │
│  │  Store      │   persist transcript  │ TUI (--tui)       │  │
│  │             │   + messages           │ JSON (--json)     │  │
│  │ SQLite      │                       │ notify (--notify) │  │
│  └─────────────┘                       │ socket (--daemon) │  │
│                                        └───────────────────┘  │
└────────────────────────────────────────────────────────────────┘
```

### Design Rules

1. **Single process.** No child processes, no sidecar daemons, no background services. One PID.
2. **Single binary.** Statically linked where possible. Runtime deps: PipeWire (system-provided on any modern Linux).
3. **Async I/O, threaded compute.** `tokio` for network/file I/O. Dedicated OS threads for audio capture and whisper inference (CPU-bound, must not block async runtime).
4. **Crash-safe state.** SQLite WAL mode. Transcript recoverable up to last flush (every 5s).
5. **No framework.** No Tauri, Electron, GTK, Qt, or any UI framework. Output is text. Display is your terminal + window manager.

---

## 2. Audio Pipeline

### 2.1 Capture: PipeWire-Native (Primary) + cpal (Fallback)

**Primary Linux path: `pipewire-rs`** (direct PipeWire bindings)

Why not cpal? On Linux, cpal routes through an ALSA compatibility layer even when PipeWire is running. This adds latency and loses access to PipeWire's graph-based routing. Direct `pipewire-rs` gives us:
- Native loopback capture via `STREAM_CAPTURE_SINK` flag (zero user config)
- Direct access to monitor ports (no `pavucontrol` setup)
- Microsecond-class latency (PipeWire's JACK-grade audio graph)
- Intelligent sink detection (handles EasyEffects and other virtual devices)

```rust
// PipeWire stream setup for system audio loopback
use pipewire as pw;

fn create_capture_stream(core: &pw::core::Core) -> pw::stream::Stream {
    let props = pw::properties::properties! {
        *pw::keys::MEDIA_TYPE => "Audio",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::STREAM_CAPTURE_SINK => "true",   // loopback capture
        *pw::keys::NODE_NAME => "cue-capture",
    };

    let stream = pw::stream::Stream::new(core, "cue-system", props)?;

    stream.connect(
        pw::stream::Direction::Input,
        None,  // auto-detect default sink monitor
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &[/* audio format params: f32, 16kHz, mono */],
    )?;

    stream
}
```

**Inside the PipeWire `process` callback:**

```rust
// Zero-copy audio path: dequeue buffer → copy into ring buffer → release
fn on_process(stream: &pw::stream::Stream, ring: &Producer<f32>) {
    if let Some(buffer) = stream.dequeue_buffer() {
        let data = buffer.datas_mut()[0];
        let samples: &[f32] = unsafe {
            std::slice::from_raw_parts(
                data.data().unwrap() as *const f32,
                data.chunk().size() as usize / std::mem::size_of::<f32>(),
            )
        };
        ring.push_slice(samples);
    }
}
```

**Critical:** PipeWire runs its own main loop on a dedicated OS thread. Communication with the rest of cue is via lock-free ring buffers (audio data) and `tokio::sync::mpsc` channels (control messages).

**Monitor source discovery:**

```rust
// Traverse PipeWire node graph to find the true terminal monitor sink
// Handles EasyEffects, virtual sinks, and complex routing
fn find_monitor_source(registry: &pw::registry::Registry) -> Option<u32> {
    // 1. Find the default sink via PipeWire metadata
    // 2. Follow links to find its .monitor port
    // 3. If EasyEffects detected, follow the chain:
    //    app → easyeffects_sink → easyeffects_source → real_sink.monitor
}
```

**Fallback: `cpal`** for non-PipeWire systems (bare ALSA, PulseAudio-only):

```rust
#[cfg(not(feature = "pipewire"))]
fn create_capture_cpal() -> cpal::Stream {
    // Standard cpal path via ALSA/PulseAudio backend
}
```

**Dual-stream capture:**

```
Thread 1 (PipeWire loop): system audio (monitor source) → system_ring_buffer
Thread 2 (PipeWire loop or cpal): microphone input → mic_ring_buffer
Thread 3 (OS): VAD reads from both buffers
Thread 4 (OS): Whisper inference
Tokio runtime: Everything else
```

**Ring buffer:** `rtrb` crate (real-time safe, SPSC, lock-free). 30-second window per channel:
```
30s × 16,000 Hz × 4 bytes = 1.92 MB per channel. Total: ~4 MB.
```

### 2.2 Voice Activity Detection

**Two-tier approach:**

Tier 1 — Energy gate (free):
```rust
fn energy_gate(samples: &[f32], threshold: f32) -> bool {
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
    rms > threshold
}
```

Tier 2 — Silero VAD (ONNX, runs only on energy-gate candidates):
```rust
struct VadState {
    energy_threshold: f32,           // default 0.01
    silero_threshold: f32,           // default 0.5
    speech_pad_ms: u32,              // 300ms
    min_speech_ms: u32,              // 250ms
    max_silence_ms: u32,             // 1000ms (configurable)
    state: VadPhase,
}
```

Output: `Utterance { samples: Vec<f32>, duration_ms: u32, channel: Channel }` via bounded `tokio::mpsc` (capacity: 16). Backpressure = drop oldest.

---

## 3. Speech-to-Text Engine

### 3.1 whisper.cpp via whisper-rs

```rust
struct SttConfig {
    model_path: PathBuf,         // ~/.local/share/cue/models/ggml-tiny.bin
    language: String,            // "en" or "auto"
    n_threads: u32,              // default: num_cpus / 2
    beam_size: u32,              // 1 = greedy (fast), 5 = beam (accurate)
    use_gpu: bool,               // auto-detect CUDA/Vulkan
}
```

| Model | Size | Inference (3s, 4T CPU) | When to use |
|---|---|---|---|
| tiny | 75 MB | ~80ms | Default. Clear meeting audio. |
| base | 142 MB | ~180ms | Accented speakers, noise. |
| small | 466 MB | ~450ms | Maximum accuracy. |

### 3.2 Streaming: VAD-Segmented Chunks

```
VAD detects speech → accumulate samples → VAD detects 1s silence →
→ utterance complete (2-15s) → send to whisper thread → emit text
```

**KV-cache optimization:** whisper.cpp supports reusing key-value states from previous chunks during autoregressive decoding. When processing sequential utterances from the same speaker, the model reuses computed states, reducing per-chunk inference time by ~30% after the first chunk.

**Latency budget:**
```
VAD detection:        ~50ms
Silence timeout:    1000ms   (configurable: vad_silence_timeout_ms)
Whisper inference:   ~100ms  (tiny, 3s chunk, 4 threads)
─────────────────────────────
End-to-end from speech end: ~1150ms
```

### 3.3 Feature-Gated Alternative: candle-whisper

```toml
[features]
default = ["whisper-cpp"]
candle = ["candle-core", "candle-transformers", "candle-nn"]
```

Pure Rust, no C++ FFI. ~2x slower but zero external dependencies. Good for musl static builds.

---

## 4. LLM Integration

### 4.1 Provider Trait

```rust
#[async_trait]
pub trait LlmProvider: Send + Sync {
    async fn stream(
        &self,
        messages: &[Message],
        config: &CompletionConfig,
    ) -> Result<Pin<Box<dyn Stream<Item = Result<String>> + Send>>>;
    fn name(&self) -> &str;
    fn max_context(&self) -> usize;
}
```

### 4.2 Implementations

**OpenAI-compatible** (covers OpenAI, Groq, Mistral, OpenRouter):
```rust
// SSE: data: {"choices":[{"delta":{"content":"token"}}]}
pub struct OpenAiCompatible { client, base_url, api_key: Secret<String>, model }
```

**Anthropic** (different SSE format):
```rust
// SSE: event: content_block_delta / data: {"delta":{"text":"token"}}
pub struct Anthropic { client, api_key: Secret<String>, model }
```

**Ollama** (local, streaming NDJSON):
```rust
// {"response":"token","done":false}
pub struct Ollama { client, endpoint: "http://localhost:11434", model }
```

### 4.3 Router with Failover

Try primary → failover chain → `Error::AllProvidersFailed`

### 4.4 API Key Management

Resolution order:
1. Environment variable (`ANTHROPIC_API_KEY`)
2. System keyring via `keyring` crate (D-Bus `org.freedesktop.Secret.Service`)
3. `.env` file at `$XDG_CONFIG_HOME/cue/.env` (mode 600)
4. Interactive prompt on first use → stored to keyring

Keys wrapped in `Secret<String>` (`secrecy` + `zeroize` crates). Never in logs, config files, or database.

---

## 5. Knowledge Base

### 5.1 Schema

SQLite + sqlite-vec at `$XDG_DATA_HOME/cue/cue.db`.

```sql
CREATE TABLE documents (
    id INTEGER PRIMARY KEY,
    name TEXT NOT NULL,
    path TEXT,
    doc_type TEXT NOT NULL,
    size_bytes INTEGER,
    chunk_count INTEGER,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);

CREATE TABLE chunks (
    id INTEGER PRIMARY KEY,
    doc_id INTEGER NOT NULL REFERENCES documents(id) ON DELETE CASCADE,
    content TEXT NOT NULL,
    chunk_index INTEGER NOT NULL,
    token_count INTEGER
);

CREATE VIRTUAL TABLE chunks_fts USING fts5(content, content=chunks, content_rowid=id);

-- sqlite-vec virtual table for vector search
CREATE VIRTUAL TABLE chunk_vec USING vec0(
    chunk_id INTEGER PRIMARY KEY,
    embedding FLOAT[384]
);
```

### 5.2 Embedding with Zero-Copy Storage

**Model:** all-MiniLM-L6-v2 (384 dims, 22MB ONNX)

```rust
use zerocopy::AsBytes;

fn store_embedding(db: &Connection, chunk_id: i64, embedding: &[f32]) -> Result<()> {
    let bytes = embedding.as_bytes(); // 384 × 4 = 1536 bytes, zero-copy
    db.execute(
        "INSERT INTO chunk_vec(chunk_id, embedding) VALUES (?1, ?2)",
        params![chunk_id, bytes],
    )?;
    Ok(())
}

fn search_similar(db: &Connection, query_emb: &[f32], top_k: usize) -> Result<Vec<Chunk>> {
    let query_bytes = query_emb.as_bytes();
    db.prepare(
        "SELECT c.content, c.doc_id, d.name,
                vec_distance_cosine(cv.embedding, ?1) as distance
         FROM chunk_vec cv
         JOIN chunks c ON c.id = cv.chunk_id
         JOIN documents d ON d.id = c.doc_id
         ORDER BY distance ASC
         LIMIT ?2"
    )?.query_map(params![query_bytes, top_k], |row| { /* ... */ })
}
```

### 5.3 Knowledge Store Trait

```rust
#[async_trait]
pub trait KnowledgeStore: Send + Sync {
    async fn ingest(&self, path: &Path, doc_type: &str) -> Result<DocumentMeta>;
    async fn search(&self, query: &str, top_k: usize) -> Result<Vec<RetrievedChunk>>;
    async fn list(&self) -> Result<Vec<DocumentMeta>>;
    async fn remove(&self, id: i64) -> Result<()>;
}
```

---

## 6. Context Engine

Assembles: system prompt + KB context (top-k RAG) + recent transcript (2 min window) + query.

Token budget: ~1500 tokens per request. Aggressive truncation. Smaller = faster = cheaper.

Trigger modes: `Auto` | `Hotkey` | `Manual`. Default is `Hotkey`.

---

## 7. System Prompts

6 built-in templates (coding, behavioral, system_design, meeting, sales, general) compiled via `include_str!`. User overrides in `$XDG_CONFIG_HOME/cue/prompts/`.

---

## 8. Session Store

SQLite tables: `sessions`, `transcript` (with FTS5), `exchanges`. Auto-title from first 3 system-channel entries.

```sql
CREATE TABLE sessions (
    id INTEGER PRIMARY KEY,
    title TEXT,
    prompt_template TEXT,
    started_at TEXT NOT NULL,
    ended_at TEXT,
    exchange_count INTEGER DEFAULT 0
);

CREATE TABLE transcript (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id),
    channel TEXT NOT NULL,       -- 'system' or 'mic'
    text TEXT NOT NULL,
    confidence REAL,
    spoken_at TEXT NOT NULL
);

CREATE VIRTUAL TABLE transcript_fts USING fts5(text, content=transcript, content_rowid=id);

CREATE TABLE exchanges (
    id INTEGER PRIMARY KEY,
    session_id INTEGER NOT NULL REFERENCES sessions(id),
    query TEXT NOT NULL,
    response TEXT NOT NULL,
    provider TEXT NOT NULL,
    model TEXT NOT NULL,
    tokens_used INTEGER,
    latency_ms INTEGER,
    created_at TEXT NOT NULL
);
```

---

## 9. Output Layer

Trait with 5 implementations:

```rust
#[async_trait]
pub trait OutputSink: Send + Sync {
    async fn emit_transcript(&self, entry: &TranscriptEntry) -> Result<()>;
    async fn emit_response(&self, response: &LlmResponse) -> Result<()>;
    async fn emit_status(&self, status: &StatusUpdate) -> Result<()>;
}
```

| Mode | Description |
|---|---|
| `StdoutOutput` | Transcript → stderr, responses → stdout (pipeable) |
| `JsonOutput` | Structured JSON on stdout |
| `NotifyOutput` | Desktop notifications via notify-send |
| `TuiOutput` | ratatui dashboard (optional feature) |
| `DaemonOutput` | Unix socket at `/run/user/$UID/cue.sock` |

---

## 10. Global Hotkeys

### 10.1 Primary: XDG Desktop Portal (`ashpd`)

```rust
use ashpd::desktop::global_shortcuts::{GlobalShortcuts, Shortcut};

async fn register_hotkeys() -> Result<()> {
    let proxy = GlobalShortcuts::new().await?;
    let session = proxy.create_session().await?;

    let shortcuts = vec![
        Shortcut::new("toggle_recording", "Toggle Recording")
            .preferred_trigger("Super+Shift+R"),
        Shortcut::new("trigger_llm", "Send to AI")
            .preferred_trigger("Super+Return"),
        Shortcut::new("cycle_prompt", "Cycle Prompt")
            .preferred_trigger("Super+Shift+P"),
    ];

    proxy.bind_shortcuts(&session, &shortcuts).await?;

    let mut stream = proxy.receive_activated().await?;
    while let Some(event) = stream.next().await {
        match event.shortcut_id().as_str() {
            "toggle_recording" => { /* toggle audio capture */ }
            "trigger_llm"      => { /* send transcript to LLM */ }
            "cycle_prompt"     => { /* cycle system prompt */ }
            _ => {}
        }
    }
    Ok(())
}
```

**Compositor support:**
- GNOME (Mutter): via `xdg-desktop-portal-gnome`
- KDE (KWin): via `xdg-desktop-portal-kde`
- Hyprland: via `xdg-desktop-portal-hyprland`
- Sway: via `xdg-desktop-portal-wlr`

### 10.2 Fallback: evdev

```rust
#[cfg(feature = "evdev-hotkeys")]
fn listen_evdev(config: &KeybindingConfig) -> Result<impl Stream<Item = Hotkey>> {
    // Requires user in 'input' group
    // Only used when portal is unavailable
}
```

### 10.3 Resolution Order

```rust
async fn setup_hotkeys(config: &KeybindingConfig) -> Result<Box<dyn HotkeyListener>> {
    // 1. Try XDG Desktop Portal (ashpd)
    if let Ok(listener) = setup_portal_hotkeys(config).await {
        return Ok(Box::new(listener));
    }
    // 2. Try compositor-specific (Hyprland IPC)
    if let Ok(listener) = setup_hyprland_hotkeys(config).await {
        return Ok(Box::new(listener));
    }
    // 3. Fall back to evdev
    Ok(Box::new(setup_evdev_hotkeys(config)?))
}
```

---

## 11. Stealth: The Honest Assessment

### 11.1 Why CLI-First Is Already 80% Stealth

Cue runs in a terminal. No custom window, no overlay, no special rendering. Screen capture tools capture windows — cue's "UI" is text in your existing terminal emulator. No dock icon, no system tray, no taskbar entry.

### 11.2 Wayland Screen Capture Reality

On Wayland, screen sharing is brokered by the compositor via the XDG Desktop Portal and piped through PipeWire. There is **NO standardized Wayland protocol** for a client to say "exclude me from capture."

### 11.3 What Actually Works Per Compositor

| Compositor | Exclusion Method | Reliability |
|---|---|---|
| **Hyprland** | `windowrulev2 = noscreencast, title:^(cue)$` | ✅ Works |
| **Sway** | wlr-layer-shell surfaces (not captured by default in some builds) | ⚠️ Partial |
| **GNOME** | No client-side exclusion | ❌ |
| **KDE** | No client-side exclusion | ❌ |
| **Niri** | Blacks out protected content by default | ✅ Works |
| **i3/X11** | X11 has no capture exclusion | ❌ Use second monitor |

### 11.4 Layered Stealth Strategy

**Layer 1 — Terminal-native (works everywhere):** Cue runs in a terminal. Position on second monitor or share specific window in screen share.

**Layer 2 — WM-specific rules (Hyprland/Niri):**
```bash
windowrulev2 = noscreencast, title:^(cue)$
windowrulev2 = float, title:^(cue)$
windowrulev2 = pin, title:^(cue)$
windowrulev2 = opacity 0.85, title:^(cue)$
alacritty --title cue -e cue start --tui
```

**Layer 3 — Notification mode:** Notifications are compositor-managed and NOT part of screen capture by default.

**Layer 4 — Daemon mode:** No visible window at all.

**Layer 5 — Process camouflage:**
```rust
fn camouflage(name: &str) {
    prctl::set_name(name).ok();  // "pipewire-pulse" in ps/top/htop
    std::fs::write("/proc/self/comm", format!("{name}\0")).ok();
}
```

---

## 12. Speaker Diarization (Roadmap: v0.5+)

**Crate:** `native-pyannote-rs` — pure Rust implementation of the Pyannote pipeline using the `burn` ML framework.

```rust
struct DiarizationEngine {
    segmenter: PyannoteSegmentation3,
    embedder: WespeakerResnet34,
    clusters: Vec<SpeakerCluster>,
}

struct SpeakerCluster {
    id: u32,
    label: String,               // "Speaker 1", or user-assigned "Interviewer"
    embeddings: Vec<Vec<f32>>,
}
```

Impact on transcript:
```
Before:  [SYS] Can you explain how you'd design a rate limiter?
After:   [Interviewer] Can you explain how you'd design a rate limiter?
```

---

## 13. File System Layout

```
~/.local/bin/
└── cue                                # binary (~4MB)

~/.local/share/cue/                    # $XDG_DATA_HOME/cue
├── cue.db                             # SQLite (sessions, KB, transcripts)
└── models/
    ├── ggml-tiny.bin                  # whisper (75MB)
    └── all-MiniLM-L6-v2.onnx         # embeddings (22MB)

~/.config/cue/                         # $XDG_CONFIG_HOME/cue
├── cue.toml                           # config (optional)
├── .env                               # API keys (mode 600)
└── prompts/                           # custom prompts (optional)
```

---

## 14. Configuration (Full Schema)

```toml
# ~/.config/cue/cue.toml
# ALL fields optional. Defaults just work.

[provider]
# default = "anthropic"
# model = "claude-sonnet-4-20250514"
# failover = ["ollama"]
# temperature = 0.3
# max_tokens = 512

[stt]
# model = "tiny"
# language = "en"
# threads = 4
# beam_size = 1
# gpu = "auto"                     # "auto", "cuda", "vulkan", "cpu"

[audio]
# backend = "pipewire"             # "pipewire" (default), "cpal" (fallback)
# system_device = "auto"
# mic_device = "auto"
# vad_sensitivity = 0.5
# vad_silence_timeout_ms = 1000
# vad_min_speech_ms = 250

[rag]
# enabled = true
# top_k = 5
# chunk_size = 512
# chunk_overlap = 64
# transcript_window_secs = 120

[trigger]
# mode = "hotkey"                  # "auto", "hotkey", "manual"

[display]
# mode = "stdout"                  # "stdout", "tui", "json", "notify", "daemon"

[stealth]
# camouflage_enabled = true
# camouflage_name = "pipewire-pulse"

[response]
# style = "concise"                # "concise", "detailed", "code-only", "bullets"

[hotkeys]
# backend = "portal"              # "portal" (XDG), "hyprland", "evdev"
# toggle_recording = "Super+Shift+R"
# trigger_llm = "Super+Return"
# cycle_prompt = "Super+Shift+P"

[export]
# auto_export = false
# format = "markdown"
# dir = "~/Documents/cue"
```

---

## 15. Crate Dependencies

```toml
[workspace]
members = ["crates/cue-core", "crates/cue-cli"]

[workspace.dependencies]
# Async
tokio = { version = "1", features = ["rt-multi-thread", "macros", "sync", "net", "fs", "signal"] }
tokio-stream = "0.1"
async-trait = "0.1"

# Audio (primary: PipeWire direct)
pipewire = "0.8"
rtrb = "0.3"
rubato = "0.15"

# Audio (fallback)
cpal = { version = "0.15", optional = true }

# STT
whisper-rs = "0.12"

# LLM
reqwest = { version = "0.12", features = ["stream", "json", "rustls-tls"] }
eventsource-stream = "0.2"
serde = { version = "1", features = ["derive"] }
serde_json = "1"

# Knowledge Base
rusqlite = { version = "0.32", features = ["bundled", "blob"] }
sqlite-vec = "0.1"
ort = { version = "2", features = ["load-dynamic"] }
tokenizers = { version = "0.20", features = ["onig"] }
ndarray = "0.16"
zerocopy = "0.8"

# Document parsing
pdf-extract = "0.7"

# CLI
clap = { version = "4", features = ["derive"] }

# TUI (optional)
ratatui = { version = "0.29", optional = true }
crossterm = { version = "0.28", optional = true }

# Config
toml = "0.8"
dirs = "5"
dotenvy = "0.15"

# Security
secrecy = { version = "0.10", features = ["serde"] }
zeroize = "1"
keyring = "3"

# Hotkeys
ashpd = "0.10"
evdev = { version = "0.12", optional = true }

# Notifications
notify-rust = "4"

# Text rendering
pulldown-cmark = "0.12"
syntect = "5"

# Stealth
prctl = "1"

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Error handling
anyhow = "1"
thiserror = "2"

# Utilities
chrono = { version = "0.4", features = ["serde"] }

[features]
default = ["tui", "pipewire"]
tui = ["ratatui", "crossterm"]
pipewire = ["dep:pipewire"]
cpal-fallback = ["dep:cpal"]
evdev-hotkeys = ["dep:evdev"]
gpu-cuda = ["whisper-rs/cuda"]
gpu-vulkan = ["whisper-rs/vulkan"]
candle = ["candle-core", "candle-transformers"]
```

---

## 16. Build & Distribution

```bash
cargo build --release && strip target/release/cue   # ~4MB
cargo build --release --target x86_64-unknown-linux-musl   # static binary
```

Install via `curl -sSL https://getcue.sh | sh` or `cargo install cue`.

---

## 17. Performance Budgets

| Metric | Target |
|---|---|
| Binary size | <5 MB |
| Startup | <100ms |
| RAM (active) | <80 MB |
| STT latency (3s chunk) | <300ms |
| LLM first token | <500ms (network) |
| KB search (10k chunks) | <10ms |

---

## 18. Development Phases

See [PROGRESS.md](./PROGRESS.md) for current status.

### v0.1 — "Pipe works" (Week 1-2)
- PipeWire audio capture (system monitor + mic)
- VAD (energy + silero)
- whisper.cpp STT
- Single LLM provider (Anthropic)
- stdout output
- `cue start`, `cue ask`, `cue devices`
- TOML config + env var keys
- `install.sh`

### v0.2 — "Actually useful" (Week 3-4)
- Knowledge base (ingest → embed → store → retrieve)
- RAG context injection
- System prompt templates
- Multi-provider (OpenAI, Ollama, Groq)
- Failover
- `cue load`, `cue kb`, `cue prompts`

### v0.3 — "Daily driver" (Week 5-7)
- TUI mode (ratatui)
- Session history + FTS search
- Daemon mode + Unix socket
- Notification output
- Global hotkeys (ashpd portal)
- Process camouflage
- Export

### v0.4 — "Ship it" (Week 8-10)
- Static musl builds
- CI/CD (GitHub Actions)
- Nix flake + AUR
- mdBook docs
- asciinema demo
- WM config examples
- Show HN

### v0.5 — "The Brain" (Future)
- Speaker diarization (native-pyannote-rs)
- Auto-prompt switching
- Cross-session vector search
- Optional `KnowledgeStore` backend swap

---

## Appendix: Why Not X?

| "Why not..." | Answer |
|---|---|
| **Tauri** | Tauri v2 has severe Wayland bugs: Error 71 protocol crashes, blank windows on compositor sync, `alwaysOnTop` ignored. A text-output CLI avoids all of this. |
| **Electron** | 200MB+, Chromium, V8 GC causes audio buffer underruns. |
| **GTK4** | Adds libgtk4 system dep. For text output, your terminal is the renderer. |
| **cpal as primary** | On Linux, cpal routes through ALSA compat even when PipeWire is running. Direct `pipewire-rs` gives native graph access, monitor source discovery, and lower latency. |
| **evdev for hotkeys** | Requires `input` group, bypasses Wayland security model. `ashpd` is the correct Wayland approach. |
| **Browser extension** | Can't capture system audio, can't run whisper.cpp locally, no RAG, no offline mode. |
| **Python** | No static binary. Dependency hell. 10x slower for audio real-time. |
| **Docker** | For a meeting assistant running live? No. |

---

*v3 — Last updated: 2026-03-19*
*Status: Pre-implementation spec*
