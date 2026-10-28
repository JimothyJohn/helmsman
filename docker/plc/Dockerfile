# tcpethernet-plc as an interop target, built from this checkout (context is
# the repo root). Runs the OpENer-sample profile so the interop suite written
# against OpENer passes unchanged against our own adapter.
FROM rust:1.99.1-slim-trixie AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p tcpethernet-plc

FROM debian:trixie-slim
COPY --from=build /src/target/release/tcpethernet-plc /tcpethernet-plc
# 44818 tcp+udp: encapsulation. 2222 udp: Class 1 I/O. 8080 tcp: web page.
EXPOSE 44818/tcp 44818/udp 2222/udp 8080/tcp
ENTRYPOINT ["/tcpethernet-plc", "--state-dir", "/state", "--http", "0.0.0.0:8080", "--profile", "opener-sample"]
