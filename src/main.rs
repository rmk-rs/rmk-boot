#![no_std]
#![no_main]

// Logging over RTT (opt-in via the `defmt` feature)
#[cfg(feature = "defmt")]
use defmt_rtt as _;

// ---------------------------------------------------------------------------
// Feature-exclusion checks
// ---------------------------------------------------------------------------
#[cfg(all(feature = "noswap", feature = "dfu_ext"))]
compile_error!("noswap and dfu_ext are mutually exclusive");

#[cfg(all(feature = "rp2040", feature = "nrf528xx"))]
compile_error!("rp2040 and nRF52 features are mutually exclusive");

#[cfg(all(feature = "nrf52840", feature = "nrf52833"))]
compile_error!("nrf52840 and nrf52833 features are mutually exclusive");

#[cfg(feature = "rp2040")]
const _: () = {
    #[cfg(not(any(
        feature = "rp2040-2mb",
        feature = "rp2040-4mb",
        feature = "rp2040-8mb",
        feature = "rp2040-16mb"
    )))]
    compile_error!(
        "No flash size feature enabled. Enable one of: rp2040-2mb, rp2040-4mb, rp2040-8mb, rp2040-16mb"
    );
    #[cfg(all(feature = "rp2040-2mb", feature = "rp2040-4mb"))]
    compile_error!("Only one flash size feature can be enabled at a time");
    #[cfg(all(feature = "rp2040-2mb", feature = "rp2040-8mb"))]
    compile_error!("Only one flash size feature can be enabled at a time");
    #[cfg(all(feature = "rp2040-2mb", feature = "rp2040-16mb"))]
    compile_error!("Only one flash size feature can be enabled at a time");
    #[cfg(all(feature = "rp2040-4mb", feature = "rp2040-8mb"))]
    compile_error!("Only one flash size feature can be enabled at a time");
    #[cfg(all(feature = "rp2040-4mb", feature = "rp2040-16mb"))]
    compile_error!("Only one flash size feature can be enabled at a time");
    #[cfg(all(feature = "rp2040-8mb", feature = "rp2040-16mb"))]
    compile_error!("Only one flash size feature can be enabled at a time");
};

// ---------------------------------------------------------------------------
// Platform modules (compile only when feature is active)
// ---------------------------------------------------------------------------
#[cfg(feature = "nrf528xx")]
mod dfu;
#[cfg(feature = "nrf528xx")]
mod nrf528xx;
#[cfg(feature = "rp2040")]
mod rp2040;

#[cfg(feature = "dfu_ext")]
mod driver;
mod led_pwm;
mod log;

// ---------------------------------------------------------------------------
// Shared imports
// ---------------------------------------------------------------------------
use core::cell::RefCell;

use cortex_m_rt::entry;
#[cfg(not(feature = "noswap"))]
use embassy_embedded_hal::flash::partition::BlockingPartition;
use embassy_sync::blocking_mutex::Mutex;
#[cfg(not(feature = "noswap"))]
use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_time::{Duration, block_for};
#[cfg(not(feature = "noswap"))]
use embedded_storage::nor_flash::{NorFlash, ReadNorFlash};

pub(crate) use log::{debug, error, info};

// ---------------------------------------------------------------------------
// Shared constants (cfg for platform-specific values)
// ---------------------------------------------------------------------------
#[cfg(all(feature = "rp2040", not(feature = "noswap")))]
const PAGE_SIZE: usize = 4096;
#[cfg(all(feature = "rp2040", not(feature = "noswap")))]
const WRITE_SIZE: usize = 1;

#[cfg(all(feature = "nrf528xx", not(feature = "noswap")))]
const PAGE_SIZE: usize = 4096;
#[cfg(all(feature = "nrf528xx", not(feature = "noswap")))]
const WRITE_SIZE: usize = 4;

#[cfg(not(feature = "noswap"))]
const STATE_ERASE_VALUE: u8 = 0xFF;

const HB_HALF_MS: u64 = 250;
const HB_CYCLES: u32 = 2;

#[cfg(not(feature = "noswap"))]
const PRE_SWAP_MS: u64 = 1000;
#[cfg(not(feature = "noswap"))]
const PRE_REVERT_BLINK_MS: u64 = 100;
#[cfg(not(feature = "noswap"))]
const PRE_REVERT_COUNT: u32 = 3;
#[cfg(not(feature = "noswap"))]
const POST_SWAP_COUNT: u32 = 5;

const DOT_MS: u64 = 150;
const DASH_MS: u64 = 450;
const INTRA_GAP_MS: u64 = 150;
const LETTER_GAP_MS: u64 = 450;
const WORD_GAP_MS: u64 = 1050;

#[cfg(not(feature = "noswap"))]
const SWAP_BREATHE_MS: u32 = 300;
#[cfg(feature = "nrf528xx")]
const DFU_BREATHE_MS: u32 = 3000;
#[cfg(feature = "nrf528xx")]
const DTAP_SIGNAL_MS: u64 = 500;
#[cfg(feature = "dfu_ext")]
const EXT_FLASH_SIZE: u32 = 8 * 1024 * 1024;

/// Swap page size: with an external DFU flash the erase unit is the 64K W25Q
/// block (see `driver::w25q`); otherwise 4K pages.
#[cfg(all(feature = "dfu_ext", not(feature = "noswap")))]
const SWAP_PAGE_SIZE: usize = driver::w25q::SWAP_PAGE_SIZE;

#[cfg(all(not(feature = "dfu_ext"), not(feature = "noswap")))]
const SWAP_PAGE_SIZE: usize = PAGE_SIZE;

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------
#[cfg(feature = "rp2040")]
fn platform_run() -> ! {
    rp2040::run()
}

#[cfg(feature = "nrf528xx")]
fn platform_run() -> ! {
    nrf528xx::run()
}

#[entry]
fn main() -> ! {
    info!("rmk-boot {} starting", env!("CARGO_PKG_VERSION"));
    // Unconditional debug! keeps the facade import (and its no-op macro stub)
    // used in every feature combination.
    debug!("logging facade active");
    platform_run()
}

// ---------------------------------------------------------------------------
// SysTick – drives LED breathing
// ---------------------------------------------------------------------------
#[cortex_m_rt::exception]
fn SysTick() {
    led_pwm::tick();
}

// ---------------------------------------------------------------------------
// Shared helper: progress reader
// ---------------------------------------------------------------------------
#[cfg(not(feature = "noswap"))]
fn current_progress<M: RawMutex, STATE: NorFlash + ReadNorFlash>(
    state: &mut BlockingPartition<'_, M, STATE>,
) -> usize {
    let mut validity = [0u8; WRITE_SIZE];
    state.read(WRITE_SIZE as u32, &mut validity).unwrap();
    if validity[0] != STATE_ERASE_VALUE {
        return usize::MAX;
    }
    let max_index = (state.capacity() - WRITE_SIZE) / WRITE_SIZE - 2;
    let mut word = [0u8; WRITE_SIZE];
    for index in 0..max_index {
        let offset = (2 + index) * WRITE_SIZE;
        state.read(offset as u32, &mut word).unwrap();
        if word[0] == STATE_ERASE_VALUE {
            return index;
        }
    }
    max_index
}

// ---------------------------------------------------------------------------
// Panic handler – SOS via PWM
// ---------------------------------------------------------------------------
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    error!("PANIC: {}", defmt::Display2Format(_info));
    loop {
        for _ in 0..3 {
            led_pwm::set_raw(true);
            block_for(Duration::from_millis(DOT_MS));
            led_pwm::set_raw(false);
            block_for(Duration::from_millis(INTRA_GAP_MS));
        }
        block_for(Duration::from_millis(LETTER_GAP_MS - INTRA_GAP_MS));

        for _ in 0..3 {
            led_pwm::set_raw(true);
            block_for(Duration::from_millis(DASH_MS));
            led_pwm::set_raw(false);
            block_for(Duration::from_millis(INTRA_GAP_MS));
        }
        block_for(Duration::from_millis(LETTER_GAP_MS - INTRA_GAP_MS));

        for _ in 0..3 {
            led_pwm::set_raw(true);
            block_for(Duration::from_millis(DOT_MS));
            led_pwm::set_raw(false);
            block_for(Duration::from_millis(INTRA_GAP_MS));
        }
        block_for(Duration::from_millis(WORD_GAP_MS - INTRA_GAP_MS));
    }
}
