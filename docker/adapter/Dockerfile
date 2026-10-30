# helmsman-adapter as an interop target, built from this checkout (context
# is the repo root). It mimics OpENer's POSIX sample, so the interop suite
# written against OpENer runs unchanged against our own adapter.
FROM rust:1.99.1-slim-trixie AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p helmsman-adapter

FROM debian:trixie-slim
COPY --from=build /src/target/release/helmsman-adapter /helmsman-adapter
# 44818 tcp+udp: encapsulation. 2222 udp: Class 1 I/O.
EXPOSE 44818/tcp 44818/udp 2222/udp
ENTRYPOINT ["/helmsman-adapter"]
