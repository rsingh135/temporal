#!/usr/bin/env bash
# Downloads the local models temporald needs into the app-support models dir.
# Safe to re-run; skips files that already exist with the pinned checksum.
# Every entry MUST carry a pinned SHA-256 — verification fails closed.
set -euo pipefail

MODELS_DIR="$HOME/Library/Application Support/temporald/models"

# relative_path|url|sha256  (sha256 is mandatory)
EMBEDDER_DIR="$MODELS_DIR/bge-small-en-v1.5"
declare -a FILES=(
    "bge-small-en-v1.5/model.onnx|https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/onnx/model.onnx|828e1496d7fabb79cfa4dcd84fa38625c0d3d21da474a00f08db0f559940cf35"
    "bge-small-en-v1.5/tokenizer.json|https://huggingface.co/BAAI/bge-small-en-v1.5/resolve/main/tokenizer.json|d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66"
    "qwen3-1.7b/Qwen3-1.7B-Q8_0.gguf|https://huggingface.co/Qwen/Qwen3-1.7B-GGUF/resolve/main/Qwen3-1.7B-Q8_0.gguf|061b54daade076b5d3362dac252678d17da8c68f07560be70818cace6590cb1a"
)

mkdir -p "$EMBEDDER_DIR" "$MODELS_DIR/qwen3-1.7b"
for entry in "${FILES[@]}"; do
    IFS='|' read -r rel url sha <<< "$entry"
    dest="$MODELS_DIR/$rel"
    # Fail closed: an entry without a real pinned digest is never downloaded.
    if [[ -z "$sha" || "$sha" == @* ]]; then
        echo "error: no pinned SHA-256 for $rel; refusing to fetch unverified" >&2
        exit 1
    fi
    if [ -f "$dest" ]; then
        if echo "$sha  $dest" | shasum -a 256 -c - >/dev/null 2>&1; then
            echo "ok: $rel"
            continue
        fi
        echo "checksum mismatch for existing $dest — re-downloading"
        rm -f "$dest"
    fi
    echo "==> fetching $rel"
    # --proto '=https' blocks a redirect from downgrading the scheme.
    curl -L --fail --proto '=https' --tlsv1.2 --retry 3 --retry-connrefused \
        --progress-bar -o "$dest.part" "$url"
    if ! echo "$sha  $dest.part" | shasum -a 256 -c - >/dev/null; then
        echo "error: checksum verification failed for $rel" >&2
        rm -f "$dest.part"
        exit 1
    fi
    mv "$dest.part" "$dest"
done
echo "models ready in $MODELS_DIR"
