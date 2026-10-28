# tcpethernet

An open-source originator (explicit-message client and I/O scanner) for CIP
over TCP/UDP on Ethernet, so an open device can discover, configure, and
exchange I/O with industrial devices that run a conformant adapter stack.

Status: early. Rust cargo workspace; `crates/tcpethernet` has the encapsulation
header codec, `ListIdentity` discovery over UDP, and property tests. `make help`
lists the dev targets. See `docs/SOURCES.md` for the review of the reference
stacks and ODVA documents this project draws on, and `TODO.md` for the open
decisions.

## Command line

```
cargo build --release
target/release/tcpethernet discover                     # ListIdentity broadcast
target/release/tcpethernet identity 10.0.0.50           # Identity attributes 1-7
target/release/tcpethernet get 10.0.0.50 0xF5.1.5       # any attribute, as hex
target/release/tcpethernet io 10.0.0.50 --config 151 --out 150 --in 100 \
    --out-len 32 --in-len 32 --rpi 50 --set 0102...     # Class 1 exclusive owner
```

`tcpethernet help` lists every flag. `io` binds UDP 2222 locally because
targets send input there.

