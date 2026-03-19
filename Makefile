.PHONY: build release run run-tui run-system run-coding run-meeting run-notify \
        install models clean test fmt lint help

BINARY := ./target/release/cue
DEV_BINARY := ./target/debug/cue

# ── Build ──────────────────────────────────────────────────────────────────────

build:
	cargo build

release:
	cargo build --release
	@strip $(BINARY) 2>/dev/null || true
	@echo "Binary: $(BINARY) ($$(du -h $(BINARY) | cut -f1))"

# ── Run ───────────────────────────────────────────────────────────────────────

run: release models
	$(BINARY) start

run-tui: release models
	$(BINARY) start --tui

run-coding: release models
	$(BINARY) start --prompt coding --tui

run-meeting: release models
	$(BINARY) start --prompt meeting --tui

run-behavioral: release models
	$(BINARY) start --prompt behavioral --tui

run-system-design: release models
	$(BINARY) start --prompt system_design --tui

run-notify: release models
	$(BINARY) start --notify --daemon

run-system-audio: release models
	$(BINARY) start --system-audio --tui

# ── Setup ─────────────────────────────────────────────────────────────────────

models:
	@if [ ! -f ~/.local/share/cue/models/ggml-tiny.bin ]; then \
		echo "Downloading whisper-tiny model..."; \
		$(BINARY) models download tiny; \
	fi

install: release
	mkdir -p ~/.local/bin
	cp $(BINARY) ~/.local/bin/cue
	@echo "Installed to ~/.local/bin/cue"
	@echo "Make sure ~/.local/bin is in your PATH"

# ── Dev ───────────────────────────────────────────────────────────────────────

dev:
	cargo build
	$(DEV_BINARY) start --prompt coding

fmt:
	cargo fmt

lint:
	cargo clippy -- -D warnings

test:
	cargo test

check:
	cargo check

clean:
	cargo clean

# ── Help ──────────────────────────────────────────────────────────────────────

help:
	@echo "cue — real-time AI meeting companion"
	@echo ""
	@echo "Setup:"
	@echo "  make install          Build release binary + copy to ~/.local/bin"
	@echo "  make models           Download whisper-tiny model (auto, one-time)"
	@echo ""
	@echo "Run:"
	@echo "  make run              Start with stdout output"
	@echo "  make run-tui          Start with full TUI (split panels)"
	@echo "  make run-coding       Coding interview mode + TUI"
	@echo "  make run-meeting      Meeting assistant mode + TUI"
	@echo "  make run-behavioral   Behavioral interview mode + TUI"
	@echo "  make run-system-design System design mode + TUI"
	@echo "  make run-system-audio Also capture system audio (requires PipeWire)"
	@echo "  make run-notify       Background daemon + desktop notifications"
	@echo ""
	@echo "Dev:"
	@echo "  make build            Debug build"
	@echo "  make release          Optimized release build"
	@echo "  make dev              Debug build + run coding mode"
	@echo "  make fmt              Format code"
	@echo "  make lint             Run clippy"
	@echo "  make test             Run tests"
	@echo "  make clean            Clean build artifacts"
