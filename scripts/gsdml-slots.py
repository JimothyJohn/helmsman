#!/usr/bin/env python3
"""Prints a profinet-io device line for a device, read from its GSDML file.

A bench aid until the box imports GSDML itself (TODO.md `profinet-controller`):
the `slots=` text of a device line (crates/helmsman-io/src/profinet.rs) names
every module by ident number and data length, which is what this reads out of
the vendor's file.

  /usr/bin/python3 -I scripts/gsdml-slots.py GSDML-V2.2-mdrive-....xml
  /usr/bin/python3 -I scripts/gsdml-slots.py UR.xml --slots 1,5,6,8,9 --name ur --if en0
  /usr/bin/python3 -I scripts/gsdml-slots.py UR.xml --list

Each module goes into the first slot the access point allows it in; `--slots`
keeps only those slots (the access point, slot 0, always stays). `--pdev` adds
the access point's interface and port submodules (0x8000, 0x8001, ...), which
a device may or may not insist on. Stdlib only; the file is parsed as data
(no external entities are fetched).
"""
import argparse
import sys
import xml.etree.ElementTree as ET

# Bytes of each GSDML data type with a fixed size; the others carry Length.
SIZES = {
    "Bit": 1, "Boolean": 1, "Integer8": 1, "Unsigned8": 1,
    "Integer16": 2, "Unsigned16": 2, "Integer32": 4, "Unsigned32": 4,
    "Integer64": 8, "Unsigned64": 8, "Float32": 4, "Float64": 8,
    "Unsigned8+Unsigned8": 2, "Float32+Unsigned8": 5, "F_MessageTrailer4Byte": 4,
    "F_MessageTrailer5Byte": 5,
}


def local(tag):
    return tag.rsplit("}", 1)[-1]


def children(node, name):
    return [c for c in node if local(c.tag) == name]


def descendants(node, name):
    return [c for c in node.iter() if local(c.tag) == name]


def data_length(io, direction):
    """Bytes of an IOData's Input or Output, or None if a type has no size."""
    total = 0
    for side in children(io, direction):
        for item in children(side, "DataItem"):
            length = item.get("Length")
            if length is not None:
                total += int(length)
            elif item.get("DataType") in SIZES:
                total += SIZES[item.get("DataType")]
            else:
                return None
    return total


def first_slot(spec):
    """The first slot of "1", "1 3", "1..4" style lists."""
    if not spec:
        return None
    return int(spec.replace("..", " ").split()[0])


def submodules(parent, pdev):
    """(subslot, ident, in, out) of a module's or access point's submodules."""
    out = []
    kinds = ["VirtualSubmoduleItem"]
    if pdev:
        kinds += ["InterfaceSubmoduleItem", "PortSubmoduleItem"]
    for kind in kinds:
        for n, sub in enumerate(descendants(parent, kind)):
            fixed = sub.get("FixedInSubslots") or sub.get("SubslotNumber")
            subslot = first_slot(fixed) if fixed else (1 + n if kind == "VirtualSubmoduleItem" else None)
            if subslot is None:
                raise SystemExit(f"{kind} {sub.get('ID')}: no subslot number")
            io = children(sub, "IOData")
            lengths = [data_length(io[0], d) if io else 0 for d in ("Input", "Output")]
            if None in lengths:
                raise SystemExit(f"{kind} {sub.get('ID')}: a data item of a type this script cannot size")
            out.append((subslot, int(sub.get("SubmoduleIdentNumber"), 0), lengths[0], lengths[1]))
    return out


def entry(slot, ident, subs):
    text = f"{slot}:{ident:#x}"
    for subslot, sub_ident, n_in, n_out in subs:
        text += f"/{subslot:#x}:{sub_ident:#x}" if subslot >= 0x8000 else f"/{subslot}:{sub_ident:#x}"
        if n_in:
            text += f":i{n_in}"
        if n_out:
            text += f":o{n_out}"
    return text


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("gsdml")
    ap.add_argument("--slots", help="keep only these slots, comma separated (slot 0 always stays)")
    ap.add_argument("--pdev", action="store_true", help="include the interface and port submodules")
    ap.add_argument("--name", default="dev", help="the device id in the map (default dev)")
    ap.add_argument("--station", help="the station name (default: the GSDML's, lowercased)")
    ap.add_argument("--if", dest="interface", default="en0", help="the network interface (default en0)")
    ap.add_argument("--list", action="store_true", help="list the modules and stop")
    args = ap.parse_args()

    root = ET.parse(args.gsdml).getroot()
    identity = descendants(root, "DeviceIdentity")[0]
    vendor, device = int(identity.get("VendorID"), 0), int(identity.get("DeviceID"), 0)
    texts = {t.get("TextId"): t.get("Value") for t in descendants(root, "Text")}
    modules = {m.get("ID"): m for m in descendants(root, "ModuleItem")}
    dap = descendants(root, "DeviceAccessPointItem")[0]

    def name_of(item):
        info = children(item, "ModuleInfo")
        ref = children(info[0], "Name") if info else []
        return texts.get(ref[0].get("TextId"), "") if ref else ""

    keep = {int(s) for s in args.slots.split(",")} if args.slots else None
    entries = [(first_slot(dap.get("FixedInSlots")) or 0, int(dap.get("ModuleIdentNumber"), 0), submodules(dap, args.pdev), name_of(dap))]
    for use in descendants(dap, "ModuleItemRef"):
        slot = first_slot(use.get("FixedInSlots") or use.get("UsedInSlots") or use.get("AllowedInSlots"))
        module = modules.get(use.get("ModuleItemTarget"))
        if slot is None or module is None:
            continue
        entries.append((slot, int(module.get("ModuleIdentNumber"), 0), submodules(module, False), name_of(module)))
    entries.sort(key=lambda e: e[0])

    interval = dap.get("MinDeviceInterval")
    print(f"# {args.gsdml.rsplit('/', 1)[-1]}: vendor {vendor:#06x} device {device:#06x}", end="")
    if interval:
        print(f", fastest cycle {int(interval) / 32:g} ms", end="")
    print()
    if args.list:
        for slot, ident, subs, name in entries:
            n_in, n_out = sum(s[2] for s in subs), sum(s[3] for s in subs)
            print(f"#  slot {slot:>2}  module {ident:#010x}  in {n_in:>4}  out {n_out:>4}  {name}")
        return
    used = {}
    for slot, ident, subs, _ in entries:
        if slot != entries[0][0] and keep is not None and slot not in keep:
            continue
        # Two modules that may both go in a slot: the first listed wins.
        used.setdefault(slot, entry(slot, ident, subs))
    station = args.station or (dap.get("DNS_CompatibleName") or "device").lower()
    cycle = ""
    if interval and int(interval) > 256:
        # The device line's cycle is a power of two of milliseconds.
        ms = 1
        while ms * 32 < int(interval):
            ms *= 2
        cycle = f" cycle_ms={ms}"
    print(
        f"device {args.name} profinet-io {station} if={args.interface} vendor={vendor:#06x} device={device:#06x}"
        f"{cycle} slots={','.join(used[s] for s in sorted(used))}"
    )


if __name__ == "__main__":
    try:
        main()
    except (OSError, ET.ParseError) as e:
        sys.exit(f"{sys.argv[0]}: {e}")
