# cue

Real-time AI meeting companion for Linux. Captures audio, transcribes with Whisper, and streams AI responses — all in your terminal.

```
[THEM] Can you walk me through your approach to designing a rate limiter?
[cue]  Consider a sliding window with Redis: O(1) per check, TTL auto-cleanup.
       Two counters: global + per-user. Lua script for atomicity across checks.
       Fixed window is simpler but allows 2x burst at boundaries...
```

No GUI, no cloud, no app. One binary. Your terminal is the UI.

---

## Quick Install

```bash
curl -sSL https://raw.githubusercontent.com/rohan/cue/main/install.sh | sh
```

Or build from source:
```bash
git clone https://github.com/rohan/cue
cd cue
cargo build --release
cp target/release/cue ~/.local/bin/
```

---

## Quick Start

```bash
# 1. Download Whisper model (~75MB)
cue models download tiny

# 2. Set API key
export ANTHROPIC_API_KEY=sk-ant-...

# 3. Start a session
cue start

# 4. Talk — cue transcribes and waits for your trigger
# Press Enter (manual mode) or type a question to trigger AI
```

---

## Key Commands

| Command | Description |
|---|---|
| `cue start` | Start live session (audio + STT + LLM) |
| `cue start --prompt coding` | Start with coding interview prompt |
| `cue start --trigger auto` | Auto-trigger on questions |
| `cue ask "query"` | Ask AI with transcript context |
| `cue listen` | Transcribe audio only (no LLM) |
| `cue load file.pdf` | Add document to knowledge base |
| `cue kb list` | List knowledge base documents |
| `cue kb search "query"` | Search knowledge base |
| `cue history list` | List past sessions |
| `cue history show <id>` | Show session transcript |
| `cue models download tiny` | Download Whisper model |
| `cue providers test` | Test API key and provider |
| `cue prompts list` | List system prompts |
| `cue devices` | List audio devices |
| `cue config` | Print current config |

---

## System Prompts

6 built-in prompts, activated with `--prompt <name>`:

| Prompt | Use case |
|---|---|
| `general` | Default helpful assistant |
| `coding` | Live coding interviews (hints, not solutions) |
| `behavioral` | STAR method for behavioral questions |
| `system_design` | Architecture interviews |
| `meeting` | Meeting summaries and action items |
| `sales` | Real-time sales conversation support |

---

## Trigger Modes

| Mode | Behavior |
|---|---|
| `manual` (default) | Press Enter or type a question |
| `auto` | Triggers after any sentence ending with `?` |

```bash
cue start --trigger manual   # default
cue start --trigger auto     # auto-trigger on questions
```

---

## Configuration

Config file: `~/.config/cue/cue.toml` (all fields optional)

```toml
[provider]
default = "anthropic"          # anthropic, openai, groq, mistral, ollama
model = "claude-sonnet-4-20250514"
temperature = 0.3
max_tokens = 512

[stt]
model = "tiny"                 # tiny, base, small
language = "en"
threads = 4

[audio]
vad_sensitivity = 0.5          # 0.0–1.0, higher = more sensitive
vad_silence_timeout_ms = 1000

[rag]
enabled = true
top_k = 5                      # chunks to retrieve from KB
transcript_window_secs = 120   # how much transcript to include

[trigger]
mode = "manual"                # manual, auto
```

API keys via environment variables or `~/.config/cue/.env`:

```bash
ANTHROPIC_API_KEY=sk-ant-...
OPENAI_API_KEY=sk-...
GROQ_API_KEY=gsk-...
```

---

## WM Integration

### Hyprland

```conf
# ~/.config/hypr/hyprland.conf

# Run cue in a floating terminal excluded from screen capture
bind = SUPER SHIFT, C, exec, alacritty --title cue -e cue start --prompt coding

# Window rules
windowrulev2 = noscreencast, title:^(cue)$
windowrulev2 = float, title:^(cue)$
windowrulev2 = pin, title:^(cue)$
windowrulev2 = move 70% 0%, title:^(cue)$
windowrulev2 = size 30% 100%, title:^(cue)$
windowrulev2 = opacity 0.85, title:^(cue)$
```

### Sway

```conf
# ~/.config/sway/config

bindsym $mod+Shift+c exec foot --title cue cue start --prompt coding

for_window [title="cue"] {
    floating enable
    sticky enable
}
```

### Generic (any WM)

```bash
# Position on second monitor, share only primary monitor in screen share
alacritty --title cue -e cue start
```

---

## Knowledge Base

Load documents to give cue context for your specific domain:

```bash
# Load documents
cue load resume.pdf
cue load system-design-notes.md
cue load interview-prep.txt

# Search
cue kb search "distributed systems"

# List
cue kb list
```

During a session, cue automatically retrieves relevant chunks to include in the AI prompt.

---

## Providers

| Provider | Env var | Notes |
|---|---|---|
| `anthropic` | `ANTHROPIC_API_KEY` | Default. Best quality. |
| `openai` | `OPENAI_API_KEY` | GPT-4o, etc. |
| `groq` | `GROQ_API_KEY` | Fast inference (llama, mixtral) |
| `mistral` | `MISTRAL_API_KEY` | Mistral models |
| `openrouter` | `OPENROUTER_API_KEY` | Any model via openrouter.ai |
| `ollama` | none | Local models, no key needed |

Configure failover:
```toml
[provider]
default = "anthropic"
failover = ["openai", "ollama"]
```

---

## Docs

- [Overview](docs/OVERVIEW.md)
- [Technical Spec](docs/TECH_SPEC.md)
- [Architecture](docs/ARCHITECTURE.md)
- [Plan](docs/PLAN.md)
- [Progress](docs/PROGRESS.md)

---

## Status

v0.1 — Core pipeline working. See [PROGRESS.md](docs/PROGRESS.md).
