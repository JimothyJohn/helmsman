# Fixtures

`panel-layout-handmade.dxf` is HAND-MADE (2026-11-21), not a vendor file.
It follows the structure of an AutoCAD R2018 ASCII DXF panel layout (HEADER,
TABLES, BLOCKS with ATTDEF, ENTITIES with INSERT / ATTRIB / SEQEND, TEXT,
MTEXT, LINE, LWPOLYLINE, an OBJECTS section, CRLF line endings) and has parts
for the library's Moxa ioLogik E1213, ifm IF6123 and O5D100 and Murrelektronik
IMPACT67 E DI8 DO8. It exists because no vendor DXF could be fetched with
redistribution terms we could check (the attempts are in the PR). The numbers
in it (positions, sizes) are invented. Public domain: do what you like with it.

Vendor files are never committed here. `tests/vendor.rs` runs over real ones
when `HELMSMAN_DXF_CACHE` names a directory holding them.
