# CDC — Claude Development CLI

Multi-session Claude Code orchestrator TUI built in Rust.

One terminal, multiple Claude Code sessions. Voice commands route tasks to workers.

## Features

### Multi-Session Orchestration
- Orchestrator + N worker panes in a single terminal
- Horizontal tiling (workers top 80%, orchestrator bottom 20%)
- Mouse click / Ctrl+1~9 for pane switching
- Ctrl+N to add workers with fuzzy directory search
- Ctrl+Z fullscreen toggle

### Voice Input (Phase 4)
- **Ctrl+R** toggle: start/stop microphone recording
- whisper-rs Large-v3 model — local-only Korean STT
- Auto-downloads model (~2.9GB) on first use
- Worker routing: "워커 1에게 테스트 해" parses and sends directly to worker 1

### Voice Correction (Phase 5)
- STT text auto-corrected via orchestrator Claude
- Sentinel markers `[CDC_CORRECT]...[/CDC_CORRECT]` for reliable extraction
- Quiescence-based detection (500ms no output = response complete)
- Fallback to raw STT text if correction fails

### Terminal Emulation
- Full VTE parser: SGR, cursor movement, scroll regions, DECAWM, alt screen save/restore
- Wide character support (Korean/CJK)
- 10K line scrollback buffer
- IME input support
- 60fps rendering

### Session Management
- Ctrl+S save session (`~/.cdc/sessions/`)
- `--restore <name>` to restore sessions
- Confirmation dialogs for quit/close

### Smart Alerts
- Permission request detection (red blinking border)
- Voice state indicators: `[REC]`, `[DL: N%]`, `[STT...]`, `[CORRECTING...]`

## Keybindings

| Key | Action |
|-----|--------|
| Ctrl+R | Voice recording toggle |
| Ctrl+N | Add worker |
| Ctrl+O | Focus orchestrator |
| Ctrl+1~9 | Focus worker N |
| Ctrl+Z | Fullscreen toggle |
| Ctrl+W | Close worker |
| Ctrl+S | Save session |
| Ctrl+Q | Quit |
| Shift+PgUp/Down | Scrollback |

## Installation

### Prerequisites
- Rust (edition 2024)
- cmake (for whisper-rs)
- `claude` CLI installed and authenticated

### Build

```bash
git clone https://github.com/UnripePlum/claude-developent-cli
cd claude-developent-cli
cargo build --release
```

### Run

```bash
./target/release/cdc
```

### Options

```
cdc [OPTIONS]

Options:
  --restore <NAME>    Restore a saved session
  --cwd <DIR>         Set working directory
  --setup             Show setup instructions
```

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `CDC_CMD` | `claude` | Command to spawn in panes |
| `CDC_VOICE_LOG` | — | Path to log STT results for debugging |
| `CDC_PTY_LOG` | — | Path to log raw PTY bytes |
| `CDC_CORRECTION_TIMEOUT_MS` | `5000` | Hard ceiling for correction wait |
| `CDC_CORRECTION_QUIESCENCE_MS` | `500` | Output quiescence threshold |

## Architecture

```
+-------------------+-------------------+-------------------+
|  Worker 1         |  Worker 2         |  Worker 3         |  80%
|  (claude)         |  (claude)         |  (claude)         |
+-----------------------------------------------------------+
|  Orchestrator (claude)                                     |  20%
|  Ctrl+R -> STT -> Correction -> Worker Routing            |
+-----------------------------------------------------------+

Voice Flow:
  Mic -> cpal -> whisper-rs (Large-v3) -> STT text
    -> Orchestrator corrects -> [CDC_CORRECT]...[/CDC_CORRECT]
    -> parse_worker_route() -> Worker N PTY
```

## Tech Stack

- **Rust** — ratatui + crossterm TUI
- **PTY** — portable-pty multiplexing
- **Audio** — cpal 0.17 (CoreAudio on macOS)
- **STT** — whisper-rs 0.16 (Large-v3, local-only)
- **Channels** — crossbeam for non-blocking event architecture

## Roadmap

| Phase | Status | Description |
|-------|--------|-------------|
| Phase 1 | Done | Single PTY + VTE parser |
| Phase 2 | Done | Multi-worker layout |
| Phase 3 | Done | Sessions, scrollback, dialogs, DECAWM |
| Phase 4 | Done | Voice input (whisper-rs + worker routing) |
| Phase 5 | Done | SelfCorrector (orchestrator-based correction) |
| Phase 6 | Planned | PromptCorrector (prompt quality improvement) |

## License

MIT
