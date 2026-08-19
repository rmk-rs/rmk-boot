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
    let is_dfu_ext = env::var("CARGO_FEATURE_DFU_EXT").is_ok();
    let is_defmt = env::var("CARGO_FEATURE_DEFMT").is_ok();

    if is_defmt {
        println!("cargo:rustc-link-arg-bins=-Tdefmt.x");
    }

    if is_nrf52840 && is_nrf52833 {
        panic!("nrf52840 and nrf52833 are mutually exclusive");
    }

    const PAGE_SIZE: usize = 4 * 1024;
    const STORAGE_SIZE: usize = 32 * 1024;

    if is_rp2040 {
        let (variant_label, variant_slug, flash_size) =
            if env::var("CARGO_FEATURE_RP2040_2MB").is_ok() {
                ("RP2040 2 MB", "rp2040-2mb", 2 * 1024 * 1024)
            } else if env::var("CARGO_FEATURE_RP2040_4MB").is_ok() {
                ("RP2040 4 MB", "rp2040-4mb", 4 * 1024 * 1024)
            } else if env::var("CARGO_FEATURE_RP2040_8MB").is_ok() {
                ("RP2040 8 MB", "rp2040-8mb", 8 * 1024 * 1024)
            } else if env::var("CARGO_FEATURE_RP2040_16MB").is_ok() {
                ("RP2040 16 MB", "rp2040-16mb", 16 * 1024 * 1024)
            } else {
                panic!("No RP2040 flash size feature enabled");
            };
        let variant_slug = if is_noswap {
            format!("{variant_slug}-noswap")
        } else if is_dfu_ext {
            format!("{variant_slug}-dfu_ext")
        } else {
            variant_slug.to_string()
        };

        let remaining = flash_size - 28 * 1024 - STORAGE_SIZE;
        const SWAP_PAGE_SIZE: usize = 64 * 1024;
        let (active_size, dfu_size) = if is_dfu_ext {
            // ACTIVE capacity must be a multiple of the swap page size
            // (embassy computes PAGE_SIZE = max(ERASE_SIZE) of both flashes)
            ((remaining / SWAP_PAGE_SIZE) * SWAP_PAGE_SIZE, 0)
        } else if is_noswap {
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
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_DFU_EXT");

        let rmk_boot_x = build_rmk_boot_x(
            variant_label,
            abs_active_offset,
            active_size as u32,
            rel_state_offset,
            0x1000,
            if is_noswap || is_dfu_ext { 0 } else { rel_dfu_offset },
            if is_noswap || is_dfu_ext { 0 } else { rel_dfu_size },
            rel_storage_offset,
            STORAGE_SIZE as u32,
            0x1000_0000, // XIP flash base
            256 * 1024,
        );
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        write_rmk_boot_x(&project_root, &variant_slug, &rmk_boot_x);
    } else if is_nrf52840 || is_nrf52833 {
        let (variant_label, variant_slug, flash_size, ram_size) = if is_nrf52840 {
            ("nRF52840", "nrf52840", 1024 * 1024, 256 * 1024)
        } else {
            ("nRF52833", "nrf52833", 512 * 1024, 128 * 1024)
        };
        // 24K bootloader — same as the RP2040 layout. Defmt debug builds need
        // 32K (~29K binary); pair the app with the generated rmk-memory.x when
        // flashing a defmt bootloader.
        let bootloader_size = if is_defmt { 32 * 1024 } else { 24 * 1024 };
        let state_size = 4 * 1024;
        let remaining = flash_size - bootloader_size - state_size - STORAGE_SIZE;
        const SWAP_PAGE_SIZE: usize = 64 * 1024;
        let (active_size, dfu_size) = if is_dfu_ext {
            ((remaining / SWAP_PAGE_SIZE) * SWAP_PAGE_SIZE, 0)
        } else if is_noswap {
            (remaining, 0)
        } else {
            ((remaining - PAGE_SIZE) / 2, (remaining - PAGE_SIZE) / 2 + PAGE_SIZE)
        };

        let abs_state_offset = bootloader_size as u32;
        let abs_active_offset = (bootloader_size + state_size) as u32;
        let abs_dfu_offset = abs_active_offset + active_size as u32;

        let (dfu_start_sym, dfu_end_sym) = if is_noswap || is_dfu_ext {
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

  RAM               : ORIGIN = 0x20000000, LENGTH = {ram_size}
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
        println!("cargo:rerun-if-env-changed=CARGO_FEATURE_DFU_EXT");

        let rmk_boot_x = build_rmk_boot_x(
            variant_label,
            abs_active_offset,
            active_size as u32,
            abs_state_offset,
            state_size as u32,
            if is_noswap || is_dfu_ext { 0 } else { abs_dfu_offset },
            if is_noswap || is_dfu_ext { 0 } else { dfu_size as u32 },
            abs_dfu_offset + dfu_size as u32,
            STORAGE_SIZE as u32,
            0x0000_0000, // flash base
            ram_size as u32,
        );
        let project_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        write_rmk_boot_x(&project_root, &nrf_variant_slug(variant_slug, is_noswap, is_dfu_ext), &rmk_boot_x);
    } else {
        panic!("No platform feature enabled (rp2040 or nrf52840 or nrf52833)");
    }

    println!("cargo:rerun-if-changed=build.rs");
}

/// Write the RMK linker script to the project root both under the generic
/// name `rmk-memory.x` and under the variant-specific `rmk-boot-{variant}-memory.x`
/// release name.
fn write_rmk_boot_x(project_root: &Path, variant_slug: &str, content: &str) {
    fs::write(project_root.join("rmk-memory.x"), content).unwrap();
    fs::write(
        project_root.join(format!("rmk-{variant_slug}-memory.x")),
        content,
    )
    .unwrap();
}

/// Variant slug for the nRF platform, appending the layout-modifying
/// `noswap`/`dfu_ext` feature when enabled.
fn nrf_variant_slug(platform: &str, is_noswap: bool, is_dfu_ext: bool) -> String {
    if is_noswap {
        format!("{platform}-noswap")
    } else if is_dfu_ext {
        format!("{platform}-dfu_ext")
    } else {
        platform.to_string()
    }
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
    flash_base: u32,
    ram_size: u32,
) -> String {
    let rel_active_offset = active_offset - flash_base;
    let state_end = state_offset + state_size;
    let active_end = rel_active_offset + active_size;
    let dfu_end = dfu_offset + dfu_size;
    let storage_end = storage_offset + storage_size;
    format!(
        "\
/* rmk-memory.x for {variant} — generated by rmk-boot/build.rs
 *
 * Provides the MEMORY layout (absolute XIP addresses) and the standard
 * embassy-boot `__bootloader_*` partition symbols (flash-relative offsets)
 * consumed by init_flash_from_linkerscript().
 *
 * If your board has a different flash size, replace this file with the
 * matching variant from the rmk-boot releases:
 *   https://github.com/rmk-rs/rmk-boot/releases
 */

MEMORY {{
  FLASH : ORIGIN = 0x{active_offset:08X}, LENGTH = {active_size}   /* ACTIVE region */
  RAM   : ORIGIN = 0x20000000, LENGTH = {ram_size}                 /* SRAM */
}}

/* Bootloader partition symbols — offsets relative to flash start.
 * The active/state/dfu names match embassy-boot's from_linkerfile_blocking(),
 * storage is an RMK extension.
 */
__bootloader_state_start   = 0x{state_offset:X};
__bootloader_state_end     = 0x{state_end:X};
__bootloader_active_start  = 0x{rel_active_offset:X};
__bootloader_active_end    = 0x{active_end:X};
__bootloader_dfu_start     = 0x{dfu_offset:X};
__bootloader_dfu_end       = 0x{dfu_end:X};
__bootloader_storage_start = 0x{storage_offset:X};
__bootloader_storage_end   = 0x{storage_end:X};
"
    )
}
