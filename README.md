# cue

Real-time AI companion that captures audio, transcribes speech, and streams AI responses. Runs as a native GTK4 glassmorphic overlay or in your terminal.

```
[THEM] Can you walk me through your approach to designing a rate limiter?
[cue]  Consider a sliding window with Redis: O(1) per check, TTL auto-cleanup.
       Two counters: global + per-user. Lua script for atomicity across checks.
       Fixed window is simpler but allows 2x burst at boundaries...
```

---

## Features

- **Native GTK4 overlay** — floating glassmorphic panel, always-on-top, drag to move/resize
- **Screen share invisible** — hidden from screen capture on Hyprland (Linux)
- **Speech-to-text** — Deepgram (cloud) or Whisper (local, offline)
- **Text-to-speech** — KittenTTS integration (small, fast, runs locally)
- **Auto-query from speech** — speak and cue automatically sends your words to the AI
- **Multiple LLM providers** — Anthropic, OpenAI, Groq, Mistral, OpenRouter, Ollama
- **Knowledge base** — load PDFs, markdown, text files for RAG context
- **Cross-platform** — Linux (full features) and macOS (core features)
- **TUI mode** — full terminal UI with split panels
- **System audio capture** — capture desktop audio via PipeWire (Linux)

---

## Quick Start

### Build from source

```bash
git clone https://github.com/rohansx/cue
cd cue
```

**Linux:**
```bash
# Install GTK4 dev libs (Ubuntu/Debian)
sudo apt install libgtk-4-dev pkg-config build-essential

# Arch
sudo pacman -S gtk4 base-devel

# Build
cargo build --release -p cue-native
```

**macOS:**
```bash
brew install gtk4 pkg-config
cargo build --release -p cue-native
```

### Run

```bash
# Native overlay (GTK4)
./target/release/cue-native

# Or TUI mode (terminal)
cargo build --release -p cue
./target/release/cue start --tui
```

### Configure

Create `~/.config/cue/cue.toml`:

```toml
[provider]
default = "openai"           # anthropic, openai, groq, mistral, ollama
model = "gpt-4o-mini"

[stt]
backend = "deepgram"         # deepgram (cloud) or whisper (local)

[tts]
enabled = true
voice = "Jasper"
python_bin = "python3"       # path to python with kittentts installed

[audio]
vad_sensitivity = 0.5        # 0.0-1.0, higher = more sensitive
vad_silence_timeout_ms = 1000

[rag]
enabled = true
top_k = 5

[trigger]
mode = "manual"              # manual, auto
```

API keys via environment variables or `~/.config/cue/.env`:

```bash
OPENAI_API_KEY=sk-...
ANTHROPIC_API_KEY=sk-ant-...
DEEPGRAM_API_KEY=...
GROQ_API_KEY=gsk-...
```

---

## Architecture

```
cue/
├── crates/
│   ├── cue-core/          # Shared library: audio, STT, TTS, LLM, config, RAG
│   └── cue-cli/           # Terminal CLI + TUI (ratatui)
├── apps/
│   ├── native/            # GTK4 glassmorphic overlay (primary UI)
│   └── overlay/           # Tauri web overlay (alternative)
```

### Platform Support

| Feature | Linux | macOS |
|---------|-------|-------|
| Native GTK4 overlay | Yes | Yes |
| Screen share hiding | Yes (Hyprland) | No |
| Microphone capture | Yes (cpal) | Yes (cpal) |
| System audio capture | Yes (PipeWire) | No |
| Speech-to-text | Yes | Yes |
| Text-to-speech | Yes | Yes |
| LLM providers | Yes | Yes |
| Knowledge base / RAG | Yes | Yes |

---

## CLI Commands

| Command | Description |
|---|---|
| `cue start` | Start live session (audio + STT + LLM) |
| `cue start --prompt coding` | Start with coding interview prompt |
| `cue start --tui` | Start with terminal UI |
| `cue start --stt deepgram` | Use Deepgram for STT |
| `cue start --system-audio` | Also capture system audio (Linux) |
| `cue ask "query"` | Ask AI with transcript context |
| `cue listen` | Transcribe audio only (no LLM) |
| `cue load file.pdf` | Add document to knowledge base |
| `cue kb list` | List knowledge base documents |
| `cue kb search "query"` | Search knowledge base |
| `cue models download tiny` | Download Whisper model |
| `cue devices` | List audio devices |
| `cue config` | Print current config |

---

## System Prompts

Built-in prompts activated with `--prompt <name>`:

| Prompt | Use case |
|---|---|
| `general` | Default helpful assistant |
| `coding` | Live coding interviews |
| `behavioral` | STAR method for behavioral questions |
| `system_design` | Architecture interviews |
| `meeting` | Meeting summaries and action items |
| `sales` | Sales conversation support |

---

## TTS Setup (Optional)

Cue uses [KittenTTS](https://github.com/KittenML/kittentts) for local text-to-speech. It's small (~25MB) and runs on CPU.

```bash
# Create a venv with Python 3.12 (required, not 3.14)
python3.12 -m venv ~/.local/share/cue/tts-venv

# Install KittenTTS
~/.local/share/cue/tts-venv/bin/pip install kittentts

# Copy the server script
mkdir -p ~/.local/share/cue/scripts
cp crates/cue-core/scripts/kittentts_server.py ~/.local/share/cue/scripts/

# Configure in cue.toml
# [tts]
# enabled = true
# voice = "Jasper"
# python_bin = "~/.local/share/cue/tts-venv/bin/python"
```

---

## Providers

| Provider | Env var | Notes |
|---|---|---|
| `anthropic` | `ANTHROPIC_API_KEY` | Claude models |
| `openai` | `OPENAI_API_KEY` | GPT-4o, etc. |
| `groq` | `GROQ_API_KEY` | Fast inference |
| `mistral` | `MISTRAL_API_KEY` | Mistral models |
| `openrouter` | `OPENROUTER_API_KEY` | Any model via openrouter.ai |
| `ollama` | none | Local models, no key needed |

---

## Hyprland Integration (Linux)

The native overlay automatically applies window rules when running on Hyprland:

- Floating, pinned, always-on-top
- Hidden from screen capture (`no_screen_share`)
- Transparent (compositor opacity)
- Drag to move, edge drag to resize

The window uses class `org.freedesktop.sysutil` to avoid detection.

For the TUI/CLI mode, add to `~/.config/hypr/hyprland.conf`:

```conf
bind = SUPER SHIFT, C, exec, alacritty --title cue -e cue start --prompt coding
windowrule = no_screen_share on, match:title cue
windowrule = float on, match:title cue
windowrule = pin on, match:title cue
windowrule = opacity 0.85, match:title cue
```

---

## License

MIT
