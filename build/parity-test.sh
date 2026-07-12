#!/usr/bin/env bash
# Three-way wire-format parity gate: the same F# fixtures must print
# byte-identically from .NET, Fable-generated Rust, and Fable-generated TS.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

export PATH="$HOME/.dotnet:$HOME/.cargo/bin:$PATH"

OUT="$(mktemp -d)"
trap 'rm -rf "$OUT"' EXIT

echo "==> generating targets"
dotnet tool restore >/dev/null
dotnet fable shared/Temporal.Parity --lang rust -o daemon/crates/temporal-parity/src >/dev/null
dotnet fable shared/Temporal.Parity --lang typescript -o ui/src/gen/parity >/dev/null

echo "==> .NET"
dotnet run --project shared/Temporal.Parity/Temporal.Parity.fsproj > "$OUT/dotnet.txt"

echo "==> Rust"
(cd daemon && cargo run -q -p temporal-parity) > "$OUT/rust.txt"

echo "==> TypeScript"
npx -y tsx ui/src/gen/parity/Program.ts > "$OUT/ts.txt"

fail=0
if ! cmp -s "$OUT/dotnet.txt" "$OUT/rust.txt"; then
    echo "PARITY FAILURE: .NET vs Rust"
    diff "$OUT/dotnet.txt" "$OUT/rust.txt" | head -20 || true
    fail=1
fi
if ! cmp -s "$OUT/dotnet.txt" "$OUT/ts.txt"; then
    echo "PARITY FAILURE: .NET vs TypeScript"
    diff "$OUT/dotnet.txt" "$OUT/ts.txt" | head -20 || true
    fail=1
fi
if grep -nE "MISMATCH|ERROR" "$OUT/dotnet.txt"; then
    echo "PARITY FAILURE: reparse errors above"
    fail=1
fi

if [ "$fail" -ne 0 ]; then exit 1; fi
echo "PARITY OK: $(wc -l < "$OUT/dotnet.txt" | tr -d ' ') lines byte-identical across .NET / Rust / TS"
