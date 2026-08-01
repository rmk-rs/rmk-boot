use std::env;
use std::fs;
use std::path::PathBuf;

fn main() {
    let out = &PathBuf::from(env::var_os("OUT_DIR").unwrap());

    let is_rp2040 = env::var("CARGO_FEATURE_RP2040").is_ok();
    let is_nrf52840 = env::var("CARGO_FEATURE_NRF52840").is_ok();

    const PAGE_SIZE: usize = 4 * 1024;

    fn fmt_size(bytes: usize) -> String {
        if bytes % (1024 * 1024) == 0 {
            format!("{}M", bytes / (1024 * 1024))
        } else if bytes % 1024 == 0 {
            format!("{}K", bytes / 1024)
        } else {
            bytes.to_string()
        }
    }

    if is_rp2040 {
        let flash_size = if env::var("CARGO_FEATURE_RP2040_2MB").is_ok() {
            2 * 1024 * 1024
        } else if env::var("CARGO_FEATURE_RP2040_4MB").is_ok() {
            4 * 1024 * 1024
        } else if env::var("CARGO_FEATURE_RP2040_8MB").is_ok() {
            8 * 1024 * 1024
        } else if env::var("CARGO_FEATURE_RP2040_16MB").is_ok() {
            16 * 1024 * 1024
        } else {
            panic!("No RP2040 flash size feature enabled");
        };

        let flash_base = 0x1000_0000u32;
        const STORAGE_SIZE: usize = 128 * 1024;
        let remaining = flash_size - 28 * 1024 - STORAGE_SIZE;
        let active_size = (remaining - PAGE_SIZE) / 2;
        let dfu_size = active_size + PAGE_SIZE;
        let active_offset = flash_base + 0x7000;
        let dfu_offset = active_offset + active_size as u32;

        let memory_x = format!(
            "\
MEMORY
{{
  BOOT2             : ORIGIN = 0x10000000, LENGTH = 0x100
  FLASH             : ORIGIN = 0x10000100, LENGTH = 24K - 0x100
  BOOTLOADER_STATE  : ORIGIN = 0x10006000, LENGTH = 4K
  ACTIVE            : ORIGIN = 0x{:08X}, LENGTH = {}
  DFU               : ORIGIN = 0x{:08X}, LENGTH = {}

  RAM               : ORIGIN = 0x20000000, LENGTH = 256K
}}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(BOOT2);
__bootloader_state_end   = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(BOOT2);

__bootloader_active_start = ORIGIN(ACTIVE) - ORIGIN(BOOT2);
__bootloader_active_end   = ORIGIN(ACTIVE) + LENGTH(ACTIVE) - ORIGIN(BOOT2);

__bootloader_dfu_start    = ORIGIN(DFU) - ORIGIN(BOOT2);
__bootloader_dfu_end      = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(BOOT2);
",
            active_offset, fmt_size(active_size),
            dfu_offset, fmt_size(dfu_size),
        );

        fs::write(out.join("memory.x"), memory_x).unwrap();
        println!("cargo:rustc-link-search={}", out.display());
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
        println!("cargo:rustc-link-arg-bins=-Tlink-rp.x");

        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RP2040_2MB");
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RP2040_4MB");
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RP2040_8MB");
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RP2040_16MB");
    } else if is_nrf52840 {
        let flash_size = 1024 * 1024;
        let bootloader_size = 24 * 1024;
        let state_size = 4 * 1024;
        let storage_size = 128 * 1024;
        let remaining = flash_size - bootloader_size - state_size - storage_size;
        let active_size = (remaining - PAGE_SIZE) / 2;
        let dfu_size = active_size + PAGE_SIZE;

        let state_offset = bootloader_size as u32;
        let active_offset = (bootloader_size + state_size) as u32;
        let dfu_offset = active_offset + active_size as u32;

        let memory_x = format!(
            "\
MEMORY
{{
  FLASH             : ORIGIN = 0x00000000, LENGTH = {bootloader_size}
  BOOTLOADER_STATE  : ORIGIN = 0x{state_offset:08X}, LENGTH = {state_size}
  ACTIVE            : ORIGIN = 0x{active_offset:08X}, LENGTH = {active_size}
  DFU               : ORIGIN = 0x{dfu_offset:08X}, LENGTH = {dfu_size}

  RAM               : ORIGIN = 0x20000000, LENGTH = 256K
}}

__bootloader_state_start   = ORIGIN(BOOTLOADER_STATE);
__bootloader_state_end     = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE);

__bootloader_active_start  = ORIGIN(ACTIVE);
__bootloader_active_end    = ORIGIN(ACTIVE) + LENGTH(ACTIVE);

__bootloader_dfu_start     = ORIGIN(DFU);
__bootloader_dfu_end       = ORIGIN(DFU) + LENGTH(DFU);
",
            bootloader_size = bootloader_size,
            state_offset = state_offset,
            state_size = state_size,
            active_offset = active_offset,
            active_size = fmt_size(active_size),
            dfu_offset = dfu_offset,
            dfu_size = fmt_size(dfu_size),
        );

        fs::write(out.join("memory.x"), memory_x).unwrap();
        println!("cargo:rustc-link-search={}", out.display());
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
    } else {
        panic!("No platform feature enabled (rp2040 or nrf52840)");
    }

    println!("cargo:rerun-if-changed=build.rs");
}
