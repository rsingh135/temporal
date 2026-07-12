# Temporal Workspace Engine

Freeze the exact state of a macOS desktop (apps, windows, Chrome tabs, VS Code/Cursor
projects, Terminal working directories) and rehydrate it later by summoning it with a
natural-language query — no manual workspace names.

## Architecture

One F# domain (`shared/Temporal.Domain`) is the single source of truth for all types,
wire codecs, and pure business logic. Fable 5 compiles it to **two targets**:

| Component | Language | Path |
|---|---|---|
| `temporald` daemon (launchd agent) | F# core → **Rust** + handwritten Rust shell | `daemon/` |
| Ephemeral UI (⌥Space panel) | F# → **TypeScript** + Tauri 2 | `ui/`, `shared/Temporal.UI` |

The handwritten Rust shell owns everything impure: macOS FFI (CGWindowList, AXUIElement,
NSWorkspace, AppleScript), SQLite + sqlite-vec storage, ONNX embeddings, llama.cpp
tagging, and the Unix-domain-socket IPC server. The Fable-generated crate
(`daemon/crates/temporal-core`) stays pure — Fable's Rust target is alpha, so no async,
no FFI, and no BCL-heavy code crosses that boundary.

## Prerequisites

- .NET SDK 8+ (user-local install works: `dotnet-install.sh --channel LTS`) on `PATH` or at `~/.dotnet`
- Rust stable toolchain (`rustup`)
- Node.js 20+

## Build

```sh
build/check.sh              # full gate: codegen, build, clippy, tests, parity, smokes
build/fable-gen.sh          # just regenerate Rust + TS from F# (after any F# change)
build/parity-test.sh        # just the three-way wire-format parity test
```

Generated code (`daemon/crates/temporal-core/src/`, `ui/src/gen/`) is gitignored —
always regenerate, never edit.

## Wire format

Serialization is written in F# itself (`Json.fs` + `Codecs.fs`) and transpiled to every
runtime, so the daemon, the UI, and the .NET tests share byte-identical JSON. CI runs a
three-way parity test over shared fixtures.
