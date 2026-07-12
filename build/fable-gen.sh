#!/usr/bin/env bash
# Regenerates all Fable outputs (F# -> Rust for the daemon, F# -> TypeScript for the UI).
# Run from anywhere; operates on the repo root.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.dotnet:$PATH"

dotnet tool restore

echo "==> Temporal.Domain -> Rust (daemon/crates/temporal-core/src)"
dotnet fable shared/Temporal.Domain --lang rust -o daemon/crates/temporal-core/src

echo "==> Temporal.Domain -> TypeScript (ui/src/gen/domain)"
dotnet fable shared/Temporal.Domain --lang typescript -o ui/src/gen/domain

echo "==> Temporal.Parity -> Rust (daemon/crates/temporal-parity/src)"
dotnet fable shared/Temporal.Parity --lang rust -o daemon/crates/temporal-parity/src

echo "==> Temporal.Parity -> TypeScript (ui/src/gen/parity)"
dotnet fable shared/Temporal.Parity --lang typescript -o ui/src/gen/parity

if [ -d shared/Temporal.UI ] && [ -f shared/Temporal.UI/Temporal.UI.fsproj ]; then
    echo "==> Temporal.UI -> TypeScript (ui/src/gen/ui)"
    dotnet fable shared/Temporal.UI --lang typescript -o ui/src/gen/ui
fi

echo "==> done"
