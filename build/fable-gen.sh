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

if [ -d shared/Temporal.UI ] && [ -f shared/Temporal.UI/Temporal.UI.fsproj ]; then
    echo "==> Temporal.UI -> TypeScript (ui/src/gen/ui)"
    dotnet fable shared/Temporal.UI --lang typescript -o ui/src/gen/ui
fi

echo "==> done"
