#!/usr/bin/env bash
# The test of scripts/ci-changes.sh: each case is a set of changed paths
# and the suites it must run. CI runs this before trusting the script.
# usage: ./scripts/test-ci-changes.sh
set -euo pipefail
cd "$(dirname "$0")/.."

fails=0
# expect "<rust> <help> <interop> <oracle>" path...
expect() {
    local want=$1
    shift
    local got
    got=$( (for p in "$@"; do printf '%s\0' "$p"; done) | ./scripts/ci-changes.sh | sed 's/^[a-z]*=//' | tr '\n' ' ' | sed 's/ $//')
    if [[ $got != "$want" ]]; then
        echo "FAIL: [$*] -> '$got', want '$want'" >&2
        fails=$((fails + 1))
    fi
}

# Nothing changed (or the diff failed): run everything.
expect "true true true true"
# Board and top-level docs changes skip all of it.
expect "false false false false" TODO.md
expect "false false false false" README.md CLAUDE.md
# A help-only change builds nothing but the help pages check.
expect "false true false false" docs/help/editor.md docs/help/editor.html docs/help/img/editor.png
expect "false true false false" TODO.md docs/help/keyboard.html
# The script that builds the pages is not on the gate's skip list.
expect "true true false false" scripts/build-help.sh
expect "false false false false" docs/ARCHITECTURE.md docs/datasheet.html docs/helmsman-datasheet.pdf docs/CNAME
expect "false false false false" "docs/odva-correspondence/2026-10 letter.pdf"
# Files tests or builds read are not docs to skip.
expect "true true false false" docs/devices/moxa-iologik-e1213-enip.map
expect "true true false false" docs/playground/index.html
expect "true true false false" docs/brand/helmsman-mark-16-yellow.svg
# Markdown inside a crate can be compiled in (tests_writer.md is).
expect "true true false false" crates/helmsman-canvas/src/tests_writer.md
expect "true true false false" crates/helmsman-plc/README.md
# A docs page and a source file together: the source wins.
expect "true true false false" TODO.md crates/helmsman-canvas/src/theme.rs
# The suites follow their crates.
expect "true true true false" crates/helmsman-proto/src/lib.rs
expect "true true true false" crates/helmsman-adapter/src/main.rs
expect "true true true false" docker/opener/Dockerfile
expect "true true true false" docker-compose.yml
expect "true true false true" crates/helmsman-iec/src/lib.rs
expect "true true false true" docker/rusty/Dockerfile
expect "true true true true" crates/helmsman-proto/src/lib.rs crates/helmsman-iec/src/eval.rs
# What CI builds with changes every suite.
expect "true true true true" Cargo.lock
expect "true true true true" Cargo.toml
expect "true true true true" rust-toolchain.toml
expect "true true true true" .github/workflows/ci.yml
expect "true true true true" scripts/ci-changes.sh
# Prefixes are anchored: look-alikes do not count.
expect "true true false false" crates/helmsman-proto-extra/src/lib.rs
expect "true true false false" xdocs/help/editor.md
expect "true true false false" docs/help
# A rename out of a crate lists the old path too (--no-renames).
expect "true true true false" crates/helmsman-proto/src/old.rs docs/help/old.rs
# Odd file names stay one path each.
expect "true true false false" $'crates/helmsman-canvas/src/a\nTODO.md'
expect "true true false false" "crates/helmsman-canvas/src/with space.rs"

if [[ $fails != 0 ]]; then
    echo "$fails case(s) failed" >&2
    exit 1
fi
echo "ci-changes: all cases pass"
