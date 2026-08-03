use std::env;
use std::fs;
use std::path::Path;

fn main() {
    let out_dir = env::var("OUT_DIR").unwrap();
    let out = Path::new(&out_dir);

    let is_rp2040 = env::var("CARGO_FEATURE_RP2040").is_ok();
    let is_nrf52840 = env::var("CARGO_FEATURE_NRF52840").is_ok();
    let is_nrf52833 = env::var("CARGO_FEATURE_NRF52833").is_ok();
    let is_noswap = env::var("CARGO_FEATURE_NOSWAP").is_ok();

    if is_nrf52840 && is_nrf52833 {
        panic!("nrf52840 and nrf52833 are mutually exclusive");
    }

    const PAGE_SIZE: usize = 4 * 1024;
    const STORAGE_SIZE: usize = 32 * 1024;

    if is_rp2040 {
        let (variant_label, flash_size) = if env::var("CARGO_FEATURE_RP2040_2MB").is_ok() {
            ("RP2040 2 MB", 2 * 1024 * 1024)
        } else if env::var("CARGO_FEATURE_RP2040_4MB").is_ok() {
            ("RP2040 4 MB", 4 * 1024 * 1024)
        } else if env::var("CARGO_FEATURE_RP2040_8MB").is_ok() {
            ("RP2040 8 MB", 8 * 1024 * 1024)
        } else if env::var("CARGO_FEATURE_RP2040_16MB").is_ok() {
            ("RP2040 16 MB", 16 * 1024 * 1024)
        } else {
            panic!("No RP2040 flash size feature enabled");
        };

        let remaining = flash_size - 28 * 1024 - STORAGE_SIZE;
        let (active_size, dfu_size) = if is_noswap {
            (remaining, 0)
        } else {
            ((remaining - PAGE_SIZE) / 2, (remaining - PAGE_SIZE) / 2 + PAGE_SIZE)
        };
        // Absolute XIP addresses
        let abs_active_offset = 0x1000_7000u32;
        let abs_dfu_offset = abs_active_offset + active_size as u32;
        let abs_state_offset = 0x1000_6000u32;
        // Flash-relative offsets (for DFU symbols)
        let flash_base = 0x1000_0000u32;
        let rel_state_offset = abs_state_offset - flash_base;
        let rel_dfu_offset = abs_dfu_offset - flash_base;
        let rel_dfu_size = dfu_size as u32;
        let rel_storage_offset = abs_dfu_offset - flash_base + dfu_size as u32;

        let memory_x = format!(
            "\
MEMORY
{{
  BOOT2             : ORIGIN = 0x10000000, LENGTH = 0x100
  FLASH             : ORIGIN = 0x10000100, LENGTH = 24K - 0x100
  BOOTLOADER_STATE  : ORIGIN = 0x10006000, LENGTH = 4K
  ACTIVE            : ORIGIN = 0x{abs_active_offset:08X}, LENGTH = {active_size}
  DFU               : ORIGIN = 0x{abs_dfu_offset:08X}, LENGTH = {dfu_size}

  RAM               : ORIGIN = 0x20000000, LENGTH = 256K
}}

__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(BOOT2);
__bootloader_state_end   = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(BOOT2);

__bootloader_active_start = ORIGIN(ACTIVE) - ORIGIN(BOOT2);
__bootloader_active_end   = ORIGIN(ACTIVE) + LENGTH(ACTIVE) - ORIGIN(BOOT2);

__bootloader_dfu_start    = ORIGIN(DFU) - ORIGIN(BOOT2);
__bootloader_dfu_end      = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(BOOT2);
"
        );

        fs::write(out.join("memory.x"), &memory_x).unwrap();
        println!("cargo:rustc-link-search={}", out.display());
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
        println!("cargo:rustc-link-arg-bins=-Tlink-rp.x");

        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RP2040_2MB");
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RP2040_4MB");
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RP2040_8MB");
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_RP2040_16MB");
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_NOSWAP");

        let rmk_boot_x = build_rmk_boot_x(
            variant_label,
            abs_active_offset,
            active_size as u32,
            rel_state_offset,
            0x1000,
            if is_noswap { 0 } else { rel_dfu_offset },
            if is_noswap { 0 } else { rel_dfu_size },
            rel_storage_offset,
            STORAGE_SIZE as u32,
        );
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        fs::write(project_root.join("rmk-boot.x"), &rmk_boot_x).unwrap();
    } else if is_nrf52840 {
        let flash_size = 1024 * 1024;
        let bootloader_size = 24 * 1024;
        let state_size = 4 * 1024;
        let remaining = flash_size - bootloader_size - state_size - STORAGE_SIZE;
        let (active_size, dfu_size) = if is_noswap {
            (remaining, 0)
        } else {
            ((remaining - PAGE_SIZE) / 2, (remaining - PAGE_SIZE) / 2 + PAGE_SIZE)
        };

        let abs_state_offset = bootloader_size as u32;
        let abs_active_offset = (bootloader_size + state_size) as u32;
        let abs_dfu_offset = abs_active_offset + active_size as u32;

        let (dfu_start_sym, dfu_end_sym) = if is_noswap {
            (
                format!("__bootloader_dfu_start     = ORIGIN(ACTIVE);"),
                format!("__bootloader_dfu_end       = ORIGIN(ACTIVE) + LENGTH(ACTIVE);"),
            )
        } else {
            (
                format!("__bootloader_dfu_start     = ORIGIN(DFU);"),
                format!("__bootloader_dfu_end       = ORIGIN(DFU) + LENGTH(DFU);"),
            )
        };

        let memory_x = format!(
            "\
MEMORY
{{
  FLASH             : ORIGIN = 0x00000000, LENGTH = {bootloader_size}
  BOOTLOADER_STATE  : ORIGIN = 0x{abs_state_offset:08X}, LENGTH = {state_size}
  ACTIVE            : ORIGIN = 0x{abs_active_offset:08X}, LENGTH = {active_size}
  DFU               : ORIGIN = 0x{abs_dfu_offset:08X}, LENGTH = {dfu_size}

  RAM               : ORIGIN = 0x20000000, LENGTH = 256K
}}

__bootloader_state_start   = ORIGIN(BOOTLOADER_STATE);
__bootloader_state_end     = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE);

__bootloader_active_start  = ORIGIN(ACTIVE);
__bootloader_active_end    = ORIGIN(ACTIVE) + LENGTH(ACTIVE);

{dfu_start_sym}
{dfu_end_sym}
"
        );

        fs::write(out.join("memory.x"), &memory_x).unwrap();
        println!("cargo:rustc-link-search={}", out.display());
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_NOSWAP");

        let rmk_boot_x = build_rmk_boot_x(
            "nRF52840",
            abs_active_offset,
            active_size as u32,
            abs_state_offset,
            state_size as u32,
            if is_noswap { 0 } else { abs_dfu_offset },
            if is_noswap { 0 } else { dfu_size as u32 },
            abs_dfu_offset + dfu_size as u32,
            STORAGE_SIZE as u32,
        );
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        fs::write(project_root.join("rmk-boot.x"), &rmk_boot_x).unwrap();
    } else if is_nrf52833 {
        let flash_size = 512 * 1024;
        let bootloader_size = 24 * 1024;
        let state_size = 4 * 1024;
        let remaining = flash_size - bootloader_size - state_size - STORAGE_SIZE;
        let (active_size, dfu_size) = if is_noswap {
            (remaining, 0)
        } else {
            ((remaining - PAGE_SIZE) / 2, (remaining - PAGE_SIZE) / 2 + PAGE_SIZE)
        };

        let abs_state_offset = bootloader_size as u32;
        let abs_active_offset = (bootloader_size + state_size) as u32;
        let abs_dfu_offset = abs_active_offset + active_size as u32;

        let (dfu_start_sym, dfu_end_sym) = if is_noswap {
            (
                format!("__bootloader_dfu_start     = ORIGIN(ACTIVE);"),
                format!("__bootloader_dfu_end       = ORIGIN(ACTIVE) + LENGTH(ACTIVE);"),
            )
        } else {
            (
                format!("__bootloader_dfu_start     = ORIGIN(DFU);"),
                format!("__bootloader_dfu_end       = ORIGIN(DFU) + LENGTH(DFU);"),
            )
        };

        let memory_x = format!(
            "\
MEMORY
{{
  FLASH             : ORIGIN = 0x00000000, LENGTH = {bootloader_size}
  BOOTLOADER_STATE  : ORIGIN = 0x{abs_state_offset:08X}, LENGTH = {state_size}
  ACTIVE            : ORIGIN = 0x{abs_active_offset:08X}, LENGTH = {active_size}
  DFU               : ORIGIN = 0x{abs_dfu_offset:08X}, LENGTH = {dfu_size}

  RAM               : ORIGIN = 0x20000000, LENGTH = 256K
}}

__bootloader_state_start   = ORIGIN(BOOTLOADER_STATE);
__bootloader_state_end     = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE);

__bootloader_active_start  = ORIGIN(ACTIVE);
__bootloader_active_end    = ORIGIN(ACTIVE) + LENGTH(ACTIVE);

{dfu_start_sym}
{dfu_end_sym}
"
        );

        fs::write(out.join("memory.x"), &memory_x).unwrap();
        println!("cargo:rustc-link-search={}", out.display());
        println!("cargo:rustc-link-arg-bins=-Tlink.x");
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_NOSWAP");

        let rmk_boot_x = build_rmk_boot_x(
            "nRF52833",
            abs_active_offset,
            active_size as u32,
            abs_state_offset,
            state_size as u32,
            if is_noswap { 0 } else { abs_dfu_offset },
            if is_noswap { 0 } else { dfu_size as u32 },
            abs_dfu_offset + dfu_size as u32,
            STORAGE_SIZE as u32,
        );
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        fs::write(project_root.join("rmk-boot.x"), &rmk_boot_x).unwrap();
    } else {
        panic!("No platform feature enabled (rp2040 or nrf52840 or nrf52833)");
    }

    println!("cargo:rerun-if-changed=build.rs");
}

fn build_rmk_boot_x(
    variant: &str,
    active_offset: u32,
    active_size: u32,
    state_offset: u32,
    state_size: u32,
    dfu_offset: u32,
    dfu_size: u32,
    storage_offset: u32,
    storage_size: u32,
) -> String {
    format!(
        "\
/* rmk-boot linker script for {variant} — generated by rmk-boot/build.rs
 *
 * Provides both the MEMORY layout (absolute XIP addresses) and
 * flash-relative DFU symbols consumed by init_flash_from_linkerscript().
 *
 * If your board has a different flash size, replace this file with the
 * matching variant from the rmk-boot releases:
 *   https://github.com/rmk-rs/rmk-boot/releases
 */

MEMORY {{
  FLASH : ORIGIN = 0x{active_offset:08X}, LENGTH = {active_size}   /* ACTIVE region */
  RAM   : ORIGIN = 0x20000000, LENGTH = 256K                       /* SRAM */
}}

/* DFU partition symbols — offsets relative to flash start */
__rmk_boot_state_offset   = 0x{state_offset:X};
__rmk_boot_state_size     = 0x{state_size:X};
__rmk_boot_dfu_offset     = 0x{dfu_offset:X};
__rmk_boot_dfu_size       = 0x{dfu_size:X};
__rmk_boot_storage_offset = 0x{storage_offset:X};
__rmk_boot_storage_size   = 0x{storage_size:X};
"
    )
}
