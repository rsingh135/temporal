# Temporal Workspace Engine

Freeze the exact state of a macOS desktop (apps, windows, Chrome tabs, VS Code/Cursor
projects, Terminal working directories) and rehydrate it later by summoning it with a
natural-language query — no manual workspace names.

## Architecture

The Rust domain crate (`daemon/crates/domain`) is the single source of truth for all
types and pure business logic. serde defines the wire format; [ts-rs](https://github.com/Aleph-Alpha/ts-rs)
exports the same types to TypeScript for the UI, so the two sides cannot drift.

| Component | Language | Path |
|---|---|---|
| `temporald` daemon (launchd agent) | Rust | `daemon/` |
| Ephemeral UI (⌥Space panel) | Tauri 2 + React/TypeScript | `ui/` |

The daemon owns everything: macOS FFI (CGWindowList, AXUIElement, AppleScript via JXA),
SQLite + sqlite-vec storage, ONNX embeddings (bge-small), embedded llama.cpp tagging
(Qwen3-1.7B, Metal), and the Unix-domain-socket IPC server (4-byte BE length-prefixed
JSON frames). The Tauri shell ferries those frames as opaque strings; the React UI
decodes them with the generated types.

## Prerequisites

- Rust stable toolchain (`rustup`)
- Node.js 20+
- cmake (for llama.cpp): `brew install cmake`

## Build

```sh
build/check.sh              # full gate: bindings, build, clippy, tests, UI typecheck+build
build/ts-gen.sh             # just regenerate TS bindings (after domain type changes)
build/fetch-models.sh       # download the embedding + LLM models (pinned sha256)
build/install-daemon.sh     # install temporald as a launchd LaunchAgent
```

Generated bindings (`ui/src/gen/`) are gitignored — always regenerate, never edit.

## Run

```sh
# daemon (or install as LaunchAgent via build/install-daemon.sh)
cd daemon && cargo run -p temporald

# UI (dev): vite serves the frontend, the shell loads it; ⌥Space toggles the panel
cd ui && npm run dev &          # dev server on :1420 (debug builds load this)
./src-tauri/target/debug/temporal-ui

# CLI probe (no UI needed)
cd daemon && cargo run -p temporald -- probe freeze
cd daemon && cargo run -p temporald -- probe query "that rust daemon work"
cd daemon && cargo run -p temporald -- probe rehydrate ws-<id>
```

## Wire format

serde over the domain types, internally tagged (`"type"` for IPC messages, `"kind"`
for node payloads), camelCase fields. The format is decode-compatible with records
written before the serde migration; `daemon/crates/domain/tests/wire_compat.rs` holds
golden fixtures from the original codec and gates every change.
