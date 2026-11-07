# ruSTy (PLC-lang/rusty, LGPL-3.0): an independent IEC 61131-3 Structured
# Text compiler, used ONLY as a test oracle for crates/helmsman-iec
# (tests/oracle.rs). Nothing from it is linked into or shipped with this repo.
# Pinned release; packages verified against the SHA-256 digests GitHub
# publishes for the v1.0.8 assets. Bump deliberately.
FROM debian:trixie-slim
ARG RUSTY_VERSION=1.0.8
ARG TARGETARCH
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates curl gcc libc6-dev \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /tmp/rusty
RUN set -eu; \
    arch="${TARGETARCH:-$(dpkg --print-architecture)}"; \
    case "$arch" in \
      arm64) compiler=547ffa0be9eff5f6f061b5b58fad5aea4f3639bf62af6fd95f6a4968fde2c854; \
             stdlib=37f885f791c2e8f2db7dda6624355fd3dcf3838fe67b42af65a6f570775bcb03 ;; \
      amd64) compiler=e31cc26327033f15ce4fbf9a2e035f77312e8aa7a648ff750547c5e985b06dfc; \
             stdlib=2b2554c684be8221be1a65e4283541c48f38047e8d756e5371cc22336c804f5f ;; \
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
