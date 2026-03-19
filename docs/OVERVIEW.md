# Cue — Real-Time AI Meeting Companion for Linux

> A single Rust binary you run from your terminal. No install. No app. No GUI. Just pipe audio → transcribe → think → respond.

```bash
# That's it. This is the whole product.
curl -sSL https://getcue.sh | sh          # one-line install (copies binary to ~/.local/bin)
cue start                                  # start listening + responding
```

---

## Why Not an App?

Every tool in this space (Cluely, Pluely, Natively, Final Round AI) ships as a 200MB+ "application" with installers, system trays, splash screens, and update mechanisms. They're solving a 10-minute problem with enterprise software architecture.

What you actually need during a meeting:
1. Something captures audio
2. Something transcribes it
3. Something sends context to an LLM
4. Something shows you the response

On Linux, 3 of those 4 already exist as system utilities. Cue is the thin glue layer that connects them into a real-time pipeline — and adds the intelligence (RAG, context, prompt management) that makes responses actually useful.

**The Unix philosophy applies perfectly here:**
- Do one thing well
- Work with text streams
- Compose with existing tools
- Small, sharp, fast

---

## How It Actually Works

### The Pipeline

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│  pw-record   │────▶│   cue stt    │────▶│  cue think   │────▶│   stdout /   │
│  (system)    │     │  (whisper)   │     │  (LLM+RAG)   │     │   overlay    │
│              │     │              │     │              │     │              │
│ already on   │     │ embedded in  │     │ embedded in  │     │ terminal /   │
│ your system  │     │ cue binary   │     │ cue binary   │     │ wofi / rofi  │
└──────────────┘     └──────────────┘     └──────────────┘     └──────────────┘
         audio stream        text stream        text stream         display
```

Or as a one-liner to show the concept:

```bash
pw-record --target="$(cue device --monitor)" - | cue stt | cue think | cue display
```

In practice `cue start` orchestrates this entire pipeline internally for performance (zero-copy audio buffers, streaming SSE), but the mental model is composable unix pipes.

### Three Modes of Running

**Mode 1: Integrated (default)** — Cue handles everything internally.
```bash
cue start
# Captures audio, transcribes, sends to LLM, shows responses
# All in one process. Ctrl+C to stop.
```

**Mode 2: Pipe-friendly** — Each stage is a separate subcommand you can compose.
```bash
# Use cue's STT on any audio file
cat meeting.wav | cue stt

# Pipe live audio from any source into cue
pw-record - | cue stt --stream | cue think --provider ollama

# Use cue's RAG without audio at all
echo "explain the rate limiting pattern from my resume" | cue think --kb ~/docs/

# Just the transcription, pipe to any tool
cue listen | tee transcript.txt | cue think >> responses.txt
```

**Mode 3: Script/daemon** — Run as a background process, interact via socket.
```bash
# Start in background
cue start --daemon

# Query from another terminal (or keybinding script)
cue ask "what did they just ask about?"
cue ask "summarize the last 5 minutes"

# Stop
cue stop
```

---

## Display: You Don't Need an Overlay Framework

The "overlay" is the part every competitor overengineers. Here's what actually works on Linux without writing a single line of GUI code:

### Option A: Transparent Terminal (Recommended)

Your terminal emulator + window manager already does everything Pluely's Tauri overlay does:

```bash
# Hyprland — make cue's terminal floating + transparent + pinned
# In hyprland.conf:
windowrulev2 = float, title:^(cue)$
windowrulev2 = opacity 0.85, title:^(cue)$
windowrulev2 = pin, title:^(cue)$
windowrulev2 = size 420 500, title:^(cue)$
windowrulev2 = move 100%-440 20, title:^(cue)$
windowrulev2 = noblur, title:^(cue)$

# Sway
for_window [title="cue"] floating enable, opacity 0.85, sticky enable, resize set 420 500, move position 1480 20

# i3
for_window [title="cue"] floating enable, sticky enable, resize set 420 500, move position 1480 20

# Run cue in a terminal with that title:
alacritty --title cue -e cue start
kitty --title cue -e cue start
foot --title cue -e cue start
```

**Stealth on Wayland (Hyprland/Niri):**
```bash
windowrulev2 = noscreencast, title:^(cue)$    # ← THIS IS THE ENTIRE STEALTH IMPLEMENTATION
```

### Option B: wofi/rofi/fuzzel Popup

```bash
cue ask "$(cue transcript --last 60s | wofi --dmenu --prompt 'Ask Cue:')" | \
  wofi --dmenu --prompt "Cue:" --lines 15
```

### Option C: Desktop Notifications

```bash
cue start --notify
# Uses notify-send. No terminal needed.
```

### Option D: Tmux/Zellij Pane

```bash
tmux split-window -h -l 40% 'cue start'
```

### Option E: Built-in TUI

```bash
cue start --tui        # ratatui dashboard
cue start              # plain stdout (default)
cue start --json       # structured JSON (for piping)
cue start --notify     # desktop notifications only
cue start --daemon     # background, interact via `cue ask`
```

---

## Installation

### One-liner

```bash
curl -sSL https://getcue.sh | sh
```

What this does:
1. Detects your architecture (x86_64 / aarch64)
2. Downloads a single ~4MB static binary from GitHub Releases
3. Copies to `~/.local/bin/cue`
4. Downloads whisper-tiny model (~75MB) to `~/.local/share/cue/models/`
5. Done. No sudo. No package manager. No dependencies.

### Alternatively

```bash
cargo install cue
nix run github:rohan/cue
yay -S cue-bin
```

### First Run

```bash
$ cue start
→ No LLM provider configured. Let's set one up.

  [1] Anthropic (Claude) — recommended
  [2] OpenAI (GPT-4o)
  [3] Ollama (local, free, no API key)
  [4] Groq (fast, free tier)
  [5] Custom endpoint

  Choice: 1

→ Enter your Anthropic API key (or set ANTHROPIC_API_KEY env var):
  sk-ant-...

→ Detecting audio devices...
  System audio: "Built-in Audio Analog Stereo Monitor" ✓
  Microphone: "Built-in Audio Analog Stereo" ✓

→ Ready. Listening...
  (Ctrl+C to stop, `cue help` for commands)
```

---

## The Intelligence Layer

### 1. Knowledge Base

```bash
cue load resume.pdf
cue load jd.txt
cue load company-notes.md
cue load ~/notes/system-design/*.md

cue kb search "rate limiting"
cue kb list
cue kb remove resume.pdf
```

When a question is detected, Cue pulls relevant chunks from YOUR documents and grounds the LLM answer in your actual experience — not generic templates.

### 2. System Prompts

```bash
cue prompts list
  1. coding-interview     — "You are helping a candidate in a live coding interview..."
  2. behavioral           — STAR method
  3. system-design        — architecture-focused
  4. meeting-assistant    — note-taker mode
  5. sales-call           — sales support
  6. general              — default

cue prompt use coding-interview
cue start --auto-prompt    # experimental: auto-switch based on transcript keywords
```

### 3. Session Memory

```bash
cue history list
cue history search "distributed consensus"
cue history export 2026-03-20 --format md > google-screen.md
```

### 4. Smart Context Window

```
┌─────────────────────────────────────────────────────┐
│ SYSTEM PROMPT                                       │  ~200 tokens
├─────────────────────────────────────────────────────┤
│ KNOWLEDGE BASE CONTEXT (top-k RAG)                  │  ~500 tokens
├─────────────────────────────────────────────────────┤
│ RECENT TRANSCRIPT (last 2 min)                      │  ~400 tokens
├─────────────────────────────────────────────────────┤
│ USER QUERY                                          │  ~50 tokens
└─────────────────────────────────────────────────────┘
               Total: ~1150 tokens
```

---

## Competitive Position

| | Cluely | Pluely | Natively | **Cue** |
|---|---|---|---|---|
| Install method | Download .dmg/.exe | Download .dmg/.deb | Download .dmg/.exe | `curl \| sh` |
| Size | ~250MB | ~10MB | ~200MB | **~4MB** |
| Linux | ❌ | ✅ (Tauri) | ❌ | **✅ native** |
| Requires GUI | ✅ | ✅ | ✅ | **❌** |
| Works over SSH | ❌ | ❌ | ❌ | **✅** |
| Local STT | ❌ | ❌ | ❌ | **✅** |
| Knowledge base | ❌ | ❌ | Basic | **✅** |
| Composable | ❌ | ❌ | ❌ | **✅** |
| Stealth | Custom code | Custom code | Custom code | **1 line WM config** |
| Cost | $20/mo | Free + API | Free + API | **Free + API** |
| Offline | ❌ | Partial | Partial | **✅** |

---

## Tech Stack

```
cue (single binary, ~4MB)
├── Audio:         pipewire-rs (primary) + cpal (fallback)
├── VAD:           silero-vad (ONNX) + energy gate
├── STT:           whisper-rs (whisper.cpp FFI)
├── LLM:           reqwest + SSE streaming
├── Knowledge:     rusqlite + sqlite-vec + ort (ONNX embeddings)
├── CLI:           clap
├── TUI:           ratatui + crossterm (optional)
├── Config:        toml + dirs (XDG)
└── IPC:           Unix domain socket (daemon mode)
```

No Tauri. No Electron. No webview. No React. No Node.js. No Python. No Docker.

---

## CLI Reference

```
cue — real-time AI meeting companion

COMMANDS:
  start                   Start listening + responding
    --provider <name>     Override default provider
    --model <name>        Override default model
    --display <mode>      stdout | tui | json | notify | daemon
    --no-mic              System audio only
    --auto-prompt         Auto-switch prompts

  stop                    Stop daemon mode

  ask <query>             One-off query with current context
  listen                  Audio capture + STT only, outputs text stream

  load <file|dir>         Add document(s) to knowledge base
  kb list                 List loaded documents
  kb search <query>       Search knowledge base
  kb remove <name>        Remove document

  history list            List past sessions
  history search <query>  Search all transcripts
  history export <id>     Export session (--format md|json|txt)

  providers list          Show configured providers
  providers add <name>    Interactive provider setup

  prompts list            List system prompts
  prompts use <name>      Switch active prompt
  prompts add <file>      Add custom prompt

  devices                 List audio devices
  models list             List downloaded STT models
  models download <name>  Download whisper model

  config                  Print current config
  config edit             Open config in $EDITOR
```

---

*Built with Rust. Runs in your terminal. Invisible by default.*
