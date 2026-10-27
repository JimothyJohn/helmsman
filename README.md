# tcpethernet

An open-source originator (explicit-message client and I/O scanner) for CIP
over TCP/UDP on Ethernet, so an open device can discover, configure, and
exchange I/O with industrial devices that run a conformant adapter stack.

Status: early. Rust cargo workspace; `crates/tcpethernet` has the encapsulation
header codec, `ListIdentity` discovery over UDP, and property tests. `make help`
lists the dev targets. See `docs/SOURCES.md` for the review of the reference
stacks and ODVA documents this project draws on, and `TODO.md` for the open
decisions.

## Not an ODVA product

This project is not affiliated with or endorsed by ODVA, Inc. EtherNet/IP and
CIP are trademarks of ODVA, Inc. This code conveys no license to the
EtherNet/IP technology or marks; anyone shipping a product that implements the
protocol needs their own ODVA Terms of Usage Agreement and Vendor ID.
