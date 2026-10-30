# ruSTy (PLC-lang/rusty, LGPL-3.0): an independent IEC 61131-3 Structured
# Text compiler, used ONLY as a test oracle for crates/helmsman-iec
# (tests/oracle.rs). Nothing from it is linked into or shipped with this repo.
# Pinned release; packages verified against the SHA-256 digests GitHub
# publishes for the v1.0.7 assets. Bump deliberately.
FROM debian:trixie-slim
ARG RUSTY_VERSION=1.0.7
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /tmp/rusty
RUN set -eu; \
    arch="${TARGETARCH:-$(dpkg --print-architecture)}"; \
    case "$arch" in \
      arm64) compiler=38dd028f4d0707942736fc246bb60f1fab1535f291c5ba3ef241e024aea98907; \
             stdlib=65c68abe84ad697923b2c32e3b4461c183a83771b416391772690ed5cc6f95b0 ;; \
      amd64) compiler=056e6dfcf9a9fd4c49476e895b2cb4d186b154a0ea26d44b7a0f2c77d3cc1e25; \
             stdlib=d75ac57957a60f992a1f348a118045cc51fe3858c49c01c248efba6038c7ba5d ;; \
      *) echo "no ruSTy package for $arch" >&2; exit 1 ;; \
    esac; \
    base="https://github.com/PLC-lang/rusty/releases/download/v${RUSTY_VERSION}"; \
    curl -fsSLo compiler.deb "$base/plc-compiler_${RUSTY_VERSION}-1_${arch}.deb"; \
    curl -fsSLo stdlib.deb "$base/plc-stdlib_${RUSTY_VERSION}-1_${arch}.deb"; \
    echo "$compiler  compiler.deb" | sha256sum -c -; \
    echo "$stdlib  stdlib.deb" | sha256sum -c -; \
    apt-get update; \
    apt-get install -y --no-install-recommends ./compiler.deb ./stdlib.deb; \
    rm -rf /var/lib/apt/lists/* /tmp/rusty
COPY run-st.sh /usr/local/bin/run-st
COPY printf.pli /usr/local/share/rusty/printf.pli
WORKDIR /work
ENTRYPOINT ["run-st"]
