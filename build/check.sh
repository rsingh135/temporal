#!/usr/bin/env bash
# Full local gate: regenerate TS bindings, build + lint + test the daemon
# workspace, typecheck + build the UI, build + lint the Tauri shell.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.cargo/bin:$PATH"

echo "==> ts bindings"
build/ts-gen.sh

echo "==> cargo build + clippy (strict) + test"
(cd daemon && cargo build --workspace \
    && cargo clippy --workspace --no-deps -- -D warnings \
    && cargo test --workspace --quiet)

echo "==> ui typecheck + build"
(cd ui && npx tsc --noEmit && npm run build)

echo "==> tauri shell build + clippy"
(cd ui/src-tauri && cargo build && cargo clippy --no-deps -- -D warnings)

echo "ALL CHECKS PASSED"
