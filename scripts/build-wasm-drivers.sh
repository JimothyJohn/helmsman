#!/usr/bin/env bash
# Rebuilds the WASM driver guests and copies each next to its source, where
# the tests read it: crates/helmsman-io/guests/<name>/ -> guests/<name>.wasm.
# The .wasm files are committed, so `cargo test` needs no wasm toolchain;
# run this after changing a guest's source and commit both.
# Needs: the wasm32-unknown-unknown target (scripts/install-dev.sh adds it).
# Idempotent: with an unchanged source and toolchain the output is the same.
set -euo pipefail
cd "$(dirname "$0")/.."
guests=crates/helmsman-io/guests
for dir in "$guests"/*/; do
    name=$(basename "$dir")
    crate=$(sed -n 's/^name = "\(.*\)"/\1/p' "$dir/Cargo.toml" | head -n 1)
    (cd "$dir" && cargo build --quiet --release --locked --target wasm32-unknown-unknown)
    built="$dir/target/wasm32-unknown-unknown/release/${crate//-/_}.wasm"
    cp "$built" "$guests/$name.wasm"
    echo "$guests/$name.wasm: $(wc -c < "$guests/$name.wasm" | tr -d ' ') bytes"
done
