---
name: verify
description: Drive the temporald daemon end-to-end over its unix socket to verify daemon-side changes against the real desktop.
---

# Verifying temporald changes

The daemon's programmatic surface is its unix socket; `temporald probe` is
the built-in client (`daemon/crates/temporald/src/probe.rs`). The Tauri UI is
just a renderer over the same frames, so daemon behavior is fully observable
here.

## Recipe

```bash
export PATH="$HOME/.cargo/bin:$PATH"
cd daemon && cargo build -p temporald

# Isolated instance: temp socket + temp db (socket paths must stay <104
# bytes — mktemp under /tmp, never a long scratchpad path). Models still
# load from ~/Library/Application Support/temporald/models/.
SMOKE=$(mktemp -d /tmp/temporal-smoke.XXXX)
./target/debug/temporald --socket "$SMOKE/t.sock" run --db "$SMOKE/t.db" \
    > "$SMOKE/daemon.log" 2>&1 &
# Startup takes ~8s (Qwen mmap); poll for $SMOKE/t.sock to appear.

./target/debug/temporald --socket "$SMOKE/t.sock" probe freeze      # captures the REAL desktop
./target/debug/temporald --socket "$SMOKE/t.sock" probe query "" --limit 5
./target/debug/temporald --socket "$SMOKE/t.sock" probe query "some prompt"
./target/debug/temporald --socket "$SMOKE/t.sock" probe rehydrate <workspace-or-group-id>
```

- `probe query` prints one JSON frame per line; pipe stdout to python/jq
  (candidate list lives in the `query-results` frame).
- Group candidates use ids like `ws-…::g0` and rehydrate directly.
- LLM enrichment (summary/tags/group labels) lands ~10-25s AFTER freeze's
  `done` frame — re-query before judging labels.
- Rehydrate opens real windows on the desktop (additive only). Check Chrome
  safety by diffing window/tab counts:
  `osascript -l JavaScript -e 'const c = Application("Google Chrome"); JSON.stringify({windows: c.windows().length, tabs: c.windows().map(w => w.tabs().length)})'`
- Cleanup: `pkill -f "temporald --socket $SMOKE"` then `rm -rf "$SMOKE"`.

## Gotchas

- Embedding-quality knobs (`GROUP_SIMILARITY_THRESHOLD`,
  `MIN_ASSEMBLED_SCORE`, `ASSEMBLED_SCORE_MARGIN`) only show their real
  behavior on a live desktop — synthetic tests pass at thresholds that fail
  on real tab soup. To tune, drop a throwaway test into
  `daemon/crates/temporald/tests/` that loads the real bge model
  (skip-if-missing pattern from `semantic_e2e.rs`) and prints pairwise
  cosine scores; delete it after.
- `build/check.sh` is the full gate; `build/ts-gen.sh` after domain type
  changes.
