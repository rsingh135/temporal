#!/usr/bin/env bash
# Regenerates the TypeScript bindings in ui/src/gen from the Rust domain
# types (ts-rs). Run after any change to daemon/crates/domain/src/types.rs.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
export PATH="$HOME/.cargo/bin:$PATH"

rm -rf "$REPO_ROOT/ui/src/gen"
(cd "$REPO_ROOT/daemon" && \
    TS_RS_EXPORT_DIR="$REPO_ROOT/ui/src/gen" cargo test -q -p temporal-domain export_bindings)
echo "bindings written to ui/src/gen"
