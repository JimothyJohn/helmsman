fn main() {
    // esp-hal's linker script for the chip (memory layout, the `.dram2_uninit`
    // section the second heap region lives in, the vector table).
    println!("cargo:rustc-link-arg=-Tlinkall.x");
}
