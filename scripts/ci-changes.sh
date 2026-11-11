#!/usr/bin/env bash
# Which CI suites a change needs. Reads the changed paths, NUL-separated
# (git diff --name-only --no-renames -z), on stdin; prints four lines for
# $GITHUB_OUTPUT:
#
#   rust=true|false     the gate's builds and tests
#   help=true|false     the help pages are current (scripts/build-help.sh --check)
#   interop=true|false  the EtherNet/IP suites (OpENer and helmsman-adapter)
#   oracle=true|false   the Structured Text differential test against ruSTy
#
# It errs towards running: no paths at all runs everything, and a path that
# is not on a skip list below runs the gate. The skip list holds only files
# no build or test reads: docs/devices/ and docs/playground/ are read by
# tests and docs/brand/ by the canvas build, so they are not on it.
# scripts/test-ci-changes.sh is its test; CI runs it first.
set -euo pipefail

# Paths the gate may skip: the root Markdown (TODO.md, README.md, ...), the
# help site, the top-level docs pages, the ODVA letters, the Pages CNAME.
skip_gate='^([^/]+\.md|docs/help/.*|docs/[^/]+\.(md|html|pdf)|docs/odva-correspondence/.*|docs/CNAME)$'
# Paths that change what CI itself runs or builds with: every suite.
everything='^(Cargo\.toml|Cargo\.lock|rust-toolchain\.toml|\.github/workflows/ci\.yml|scripts/ci-changes\.sh)$'
interop_paths='^(crates/helmsman-proto/|crates/helmsman-adapter/|docker/opener/|docker/adapter/|docker-compose\.yml$|scripts/test-interop\.sh$)'
oracle_paths='^(crates/helmsman-iec/|docker/rusty/|scripts/test-oracle\.sh$)'
# The help pages are built from docs/help/*.md and from the code (the
# shortcut table and the starters), so the check runs whenever the gate
# builds, and for a help-only change the gate would otherwise skip.
help_paths='^(docs/help/|scripts/build-help\.sh$)'

rust=false
help=false
interop=false
oracle=false
seen=0
while IFS= read -r -d '' path; do
    seen=1
    if [[ $path =~ $everything ]]; then
        rust=true
        interop=true
        oracle=true
        continue
    fi
    [[ $path =~ $skip_gate ]] || rust=true
    [[ $path =~ $help_paths ]] && help=true
    [[ $path =~ $interop_paths ]] && interop=true
    [[ $path =~ $oracle_paths ]] && oracle=true
done

if [[ $seen == 0 ]]; then
    rust=true
    interop=true
    oracle=true
fi

[[ $rust == true ]] && help=true

echo "rust=$rust"
echo "help=$help"
echo "interop=$interop"
echo "oracle=$oracle"
