# CODESYS licensing, and how to pay nothing

Checked against the CODESYS Store and help pages on 2026-10-29. Prices move;
the store page is the source of truth.

## Short version

| Use | Cost | Catch |
|---|---|---|
| CODESYS Development System (the IDE) | Free | Windows only. You accept the EULA when you install it. |
| Pi runtime on the bench, unlicensed (demo mode) | Free | Stops after **2 hours**. You restart it by hand. Otherwise fully functional, EtherNet/IP included. |
| Pi runtime, licensed | €55 + VAT (EU store) / $66 (US store), per device | It's a **private licence**. "Commercial use is not permitted" on Pi + CODESYS. |
| CODESYS on anything you sell or deploy for a customer | Paid, every device | Application-based licences: Basic S €29 up to Standard L €199, plus extra tiers for communication, visualisation and motion. |
| Our own stack (`helmsman-proto`, `helmsman-plc`) plus OpENer | Free | MIT (ours) and adapted BSD (OpENer). ODVA riders apply to products, see below. |

If you don't want to pay anyone, here's how: use CODESYS only as a bench
interop target in demo mode, and ship our own code.

## What's in each piece

**Development System.** Free from the CODESYS Store, article 1101000000,
listed at $0.00. You can put it on as many workstations as you like; there's
no copy protection. Store items marked "Free" have no licence check. Paid
add-ons such as the Professional Developer Edition ($660 per user per year)
are optional, and nothing in this repo needs them.

**CODESYS Control for Raspberry Pi SL** (article 2302000009, 64-bit runtime
since 4.10, Pi 5 since 4.11):

- Demo mode is the default. It "runs for two hours without functional
  limitations before shut down," and then you restart it manually. EtherNet/IP
  Scanner and Adapter are part of the runtime, so the interop setup in
  `docs/interop/codesys-pi.md` works in demo mode with no purchase.
- The €55/$66 licence buys you no more two-hour cutoff. It does **not** buy
  you commercial use. The store says the Pi + CODESYS combination is for
  private, test and teaching use, and that commercial use needs
  application-based licences.
- The licence lives in a soft container on the Pi (bound to that device) or
  on a CODESYS Key USB dongle (movable between controllers). Activation needs
  internet access.

**Application-based licences** (the commercial path). These licence the
application rather than the hardware. Tiers set the I/O channel count,
application size and fieldbus instances: Basic S (64 I/O, 512 kB) from €29,
Basic M from €69, Basic L from €89, Standard S from €119, Standard M from €139,
Standard L from €199. Higher tiers exist. You can upgrade between tiers but
not downgrade. Each store licence covers exactly one device.

I haven't verified which tier an EtherNet/IP Adapter project needs. The
standalone "EtherNet/IP Adapter SL" product is not compatible with
application-based licences, which suggests the communication tiers cover it.
Check the store before quoting a customer.

## Ways to stay at $0

1. **Bench only, demo mode.** Use the Pi as an interop target the way
   `codesys-pi.md` describes. Two hours is longer than any interop run we
   have. Restart the runtime when it stops.
2. **Skip CODESYS.** `make opener-up` gives an adapter in CI for free, and
   `helmsman-plc` is our own soft PLC and adapter, MIT. Anything a customer
   runs should be this, not CODESYS.
3. **Don't automate around the demo limit.** You could put a watchdog on the
   Pi that restarts the runtime every two hours. I haven't read the runtime
   EULA closely enough to say whether that's allowed, and you'd never ship it.
   Treat demo mode as bench-only.

## The licence CODESYS can't waive: ODVA

Free code is not the same as free to sell. Whatever runtime you use, a
**product** that speaks EtherNet/IP needs its maker to have an ODVA Terms of
Usage Agreement and a Vendor ID. The name "EtherNet/IP" and "CIP" are ODVA
trademarks. Open-source code, our own included, doesn't change either point.
`docs/SOURCES.md` ("Licensing, in plain terms") has the detail. That's the
one cost you can't engineer around if the goal is selling hardware.

## Sources

- [CODESYS Control for Raspberry Pi SL, store page](https://store.codesys.com/en/codesys-control-for-raspberry-pi-sl.html)
  (price, demo mode, private-licence restriction, fieldbuses)
- [Data sheet, CODESYS Control for Raspberry Pi SL](https://store.codesys.com/media/n98_media_assets/files/2302000009-F/4/CODESYS%20Control%20for%20Raspberry%20Pi%20SL_en.pdf)
- [CODESYS Development System V3, US store](https://us.store.codesys.com/codesys.html) ($0.00 listing)
- [CODESYS Professional Developer Edition](https://us.store.codesys.com/codesys-professional-developer-edition.html)
- [Runtime application-based licences](https://store.codesys.com/en/licenses/runtime.html) (tiers and prices)
- [Licensing models, CODESYS help](https://content.helpme-codesys.com/en/Lizenzierung/_license_types.html)
- [CODESYS EtherNet/IP Adapter SL, US store](https://us.store.codesys.com/codesys-ethernet-ip-adapter-sl.html)
