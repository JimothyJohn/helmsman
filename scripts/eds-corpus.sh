#!/usr/bin/env bash
# Collects the vendor EDS files that are not committed here, checks each
# file's SHA-256, and runs crates/helmsman-io/tests/eds_corpus.rs against
# them:
#   - OpENer's sample application EDS, fetched at the pinned commit the
#     Docker interop target builds (docker/opener/Dockerfile);
#   - Moxa's ioLogik E1213 EDS (v2.0), copied from $E1213_EDS (default: the
#     file at the repo root, where Nick keeps it untracked). Its terms don't
#     say it may be redistributed (TODO.md `e1213-eds-license`), so it is
#     never committed. Without it, the E1213 half of the test is skipped.
# Provenance in docs/SOURCES.md ("Device sources"). Data only: nothing in them
# is executed.
#
# Needs: curl, shasum (or sha256sum), cargo, network on the first run.
# Idempotent: files already present with the right hash are kept.
# Corpus location: $HELMSMAN_EDS_CORPUS (default target/eds-corpus).
set -euo pipefail
cd "$(dirname "$0")/.."

corpus="${HELMSMAN_EDS_CORPUS:-$PWD/target/eds-corpus}"
mkdir -p "$corpus"

sha256() {
  if command -v sha256sum >/dev/null; then sha256sum "$1"; else shasum -a 256 "$1"; fi | cut -d' ' -f1
}

opener_ref=0e1ab7ee5c272950c0dc8daf7793008d7a518912
opener_sha=0e0da5ddd62f571ea319dc29254be08cac5134236b389174983d80f0e370760f
e1213_name=moxa-iologik-e1213-e1213-t-eds-file-software-package-v2.0.eds
e1213_sha=3895fbca8bb036c1b31a9d6b68dea69eca6892fe0a6bce7409ab76311cfd470d

dest="$corpus/opener_sample_app.eds"
if [ ! -f "$dest" ] || [ "$(sha256 "$dest")" != "$opener_sha" ]; then
  curl -fsSL --retry 3 -o "$dest.part" \
    "https://raw.githubusercontent.com/EIPStackGroup/OpENer/$opener_ref/data/opener_sample_app.eds"
  got="$(sha256 "$dest.part")"
  if [ "$got" != "$opener_sha" ]; then
    rm -f "$dest.part"
    echo "SHA-256 mismatch for opener_sample_app.eds: got $got, want $opener_sha" >&2
    exit 1
  fi
  mv "$dest.part" "$dest"
fi

src="${E1213_EDS:-$PWD/$e1213_name}"
if [ -f "$src" ]; then
  got="$(sha256 "$src")"
  if [ "$got" != "$e1213_sha" ]; then
    echo "$src: SHA-256 $got, want $e1213_sha (another revision of the EDS?" \
      "docs/devices/moxa-iologik-e1213.md cites v2.0's lines)" >&2
    exit 1
  fi
  cp "$src" "$corpus/$e1213_name"
else
  echo "no E1213 EDS at $src (set E1213_EDS=PATH): checking OpENer's only" >&2
fi

HELMSMAN_EDS_CORPUS="$corpus" cargo test --locked -p helmsman-io --test eds_corpus -- --nocapture
