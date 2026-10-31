#!/bin/sh
# Compile Structured Text from stdin with ruSTy and run it; the program's
# stdout is ours. Compile errors go to stderr with exit 2, a crash exits 3.
# Flags mirror ruSTy's own lit tests (tests/lit/lit.cfg upstream).
set -eu
dir=$(mktemp -d)
cat > "$dir/prog.st"
libdir=$(dirname "$(ls /usr/lib/*-linux-gnu/libiec61131std.so)")
if ! plc "$dir/prog.st" \
    -liec61131std -L"$libdir" \
    -i "/usr/share/plc/include/*.st" \
    -i /usr/local/share/rusty/printf.pli \
    --linker=cc -o "$dir/prog" >"$dir/compile.log" 2>&1; then
    cat "$dir/compile.log" >&2
    exit 2
fi
LD_LIBRARY_PATH="$libdir" "$dir/prog" || exit 3
