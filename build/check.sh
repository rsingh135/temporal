#!/usr/bin/env bash
# Full local gate: regenerate Fable outputs, build + lint the Rust workspace,
# run .NET tests, run the three-way wire parity test, run both smokes.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.dotnet:$HOME/.cargo/bin:$PATH"

build/fable-gen.sh

# Generated crates (temporal-core, temporal-parity) must COMPILE — that's the
# alpha-Fable tripwire — but style lints only apply to handwritten crates.
echo "==> cargo build (workspace, includes generated crates)"
(cd daemon && cargo build --workspace)

echo "==> cargo clippy (handwritten crates, strict)"
(cd daemon && cargo clippy -p temporald -p temporal-ipc -p temporal-storage --no-deps -- -D warnings)

echo "==> cargo test"
(cd daemon && cargo test --workspace --quiet)

echo "==> dotnet test"
dotnet test shared/Temporal.Domain.Tests/Temporal.Domain.Tests.fsproj -v q

echo "==> parity"
build/parity-test.sh

echo "==> ts smoke"
npx -y tsx build/m1-smoke.ts

echo "==> ui build (vite + tauri shell)"
(cd ui && npm run build)
(cd ui/src-tauri && cargo build && cargo clippy --no-deps -- -D warnings)

echo "ALL CHECKS PASSED"
