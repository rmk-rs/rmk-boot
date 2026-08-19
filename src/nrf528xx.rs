use embassy_boot::BootLoaderConfig;
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::pwm::{Prescaler, SimpleConfig, SimplePwm};
#[cfg(feature = "dfu_ext")]
use embassy_nrf::spim::Spim;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use static_cell::StaticCell;

use super::*;

type FlashMutex = Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>;
static FLASH_MUTEX: StaticCell<FlashMutex> = StaticCell::new();

#[cfg(feature = "dfu_ext")]
type ExtFlashMutex = Mutex<NoopRawMutex, RefCell<crate::dfu::ExtFlash>>;
#[cfg(feature = "dfu_ext")]
static EXT_MUTEX: StaticCell<ExtFlashMutex> = StaticCell::new();

#[cfg(feature = "dfu_ext")]
embassy_nrf::bind_interrupts! {
    pub(crate) struct ExtFlashIrqs {
        TWISPI0 => embassy_nrf::spim::InterruptHandler<embassy_nrf::peripherals::TWISPI0>;
    }
}

pub fn run() -> ! {
    let mut cfg = embassy_nrf::config::Config::default();
    cfg.debug = embassy_nrf::config::Debug::NotConfigured;
    let _p = embassy_nrf::init(cfg);
    let p = unsafe { embassy_nrf::Peripherals::steal() };

    // ── SysTick ──
    let mut syst = unsafe { cortex_m::Peripherals::steal() }.SYST;
    syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
    syst.set_reload(64_000_000 / 1000 - 1);
    syst.enable_counter();
    syst.enable_interrupt();

    // ── PWM LED ──
    let mut pwm_cfg = SimpleConfig::default();
    pwm_cfg.prescaler = Prescaler::Div16;
    pwm_cfg.max_duty = 255;
    // for different LED pin, change p.P0_15 to the desired pin, e.g., p.P0_13
    // p.PWM0 can stay as is.
    let pwm = SimplePwm::new_1ch(p.PWM0, p.P0_15, &pwm_cfg);
    pwm.enable();
    led_pwm::init(pwm);

    let flash_mutex: &'static _ = FLASH_MUTEX.init(Mutex::new(RefCell::new(Nvmc::new(p.NVMC))));

    // ── Phase 1: Double-tap (similar to Adafruit BL, RAM 0x20007F7C) ──
    //
    // Two consecutive NRST resets within ~500ms enter DFU mode instead of
    // normal boot.
    use embassy_nrf::pac::POWER;
    const DBL_MEM: *mut u32 = 0x20007F7C as *mut u32;
    const DBL_MAGIC: u32 = 0x005A1AD5;

    let reset_pin = POWER.resetreas().read().resetpin();
    let magic = unsafe { core::ptr::read_volatile(DBL_MEM) };

    if reset_pin && magic == DBL_MAGIC {
        info!("double tap detected: entering USB DFU mode");
        unsafe { core::ptr::write_volatile(DBL_MEM, 0) };

        #[cfg(feature = "dfu_ext")]
        {
            use embassy_nrf::gpio::{Level, Output};

            let mut spi_cfg = embassy_nrf::spim::Config::default();
            spi_cfg.frequency = embassy_nrf::spim::Frequency::M32;

            let spi = Spim::new(p.TWISPI0, ExtFlashIrqs, p.P0_17, p.P0_20, p.P0_22, spi_cfg);
            let cs = Output::new(
                p.P0_24,
                Level::High,
                embassy_nrf::gpio::OutputDrive::Standard,
            );

            let ext_flash = crate::driver::w25q::W25qNorFlash::<_, _, { 64 * 1024 }>::new(
                spi,
                cs,
                EXT_FLASH_SIZE,
            );
            let ext_mutex: &'static _ = EXT_MUTEX.init(Mutex::new(RefCell::new(ext_flash)));
            crate::dfu::run_dfu_usb_ext(flash_mutex, ext_mutex, EXT_FLASH_SIZE);
        }

        #[cfg(not(feature = "dfu_ext"))]
        {
            crate::dfu::run_dfu_usb(flash_mutex);
        }
    } else if reset_pin {
        unsafe { core::ptr::write_volatile(DBL_MEM, DBL_MAGIC) };
        led_pwm::set_raw(false);
        block_for(Duration::from_millis(DTAP_SIGNAL_MS));
        led_pwm::set_raw(true);
    } else if magic == DBL_MAGIC {
        unsafe { core::ptr::write_volatile(DBL_MEM, 0) };
    }

    // Phase 2 — normal boot flow
    #[cfg(not(feature = "dfu_ext"))]
    let config = BootLoaderConfig::from_linkerfile_blocking(flash_mutex, flash_mutex, flash_mutex);
    #[cfg(not(feature = "dfu_ext"))]
    let active_offset = config.active.offset();

    #[cfg(feature = "dfu_ext")]
    let ext_mutex: &'static ExtFlashMutex = {
        use embassy_nrf::gpio::{Level, Output};

        let mut spi_cfg = embassy_nrf::spim::Config::default();
        spi_cfg.frequency = embassy_nrf::spim::Frequency::M32;

        // Default SPI pins — change if your board is wired differently
        let spi = Spim::new(p.TWISPI0, ExtFlashIrqs, p.P0_17, p.P0_20, p.P0_22, spi_cfg);
        let cs = Output::new(
            p.P0_24,
            Level::High,
            embassy_nrf::gpio::OutputDrive::Standard,
        );

        let ext_flash =
            crate::driver::w25q::W25qNorFlash::<_, _, { 64 * 1024 }>::new(spi, cs, EXT_FLASH_SIZE);
        EXT_MUTEX.init(Mutex::new(RefCell::new(ext_flash)))
    };

    #[cfg(feature = "dfu_ext")]
    let config = {
        let internal_cfg =
            BootLoaderConfig::from_linkerfile_blocking(flash_mutex, flash_mutex, flash_mutex);
        BootLoaderConfig {
            active: internal_cfg.active,
            dfu: BlockingPartition::new(ext_mutex, 0, EXT_FLASH_SIZE),
            state: internal_cfg.state,
        }
    };

    #[cfg(feature = "dfu_ext")]
    let active_offset = config.active.offset();

    info!(
        "nrf52: active=0x{:08x}, state=0x{:08x}, dfu mode = {}",
        active_offset,
        config.state.offset(),
        cfg!(feature = "dfu_ext")
    );

    #[cfg(feature = "noswap")]
    {
        info!("noswap: booting ACTIVE directly");
        block_for(Duration::from_millis(HB_HALF_MS));
        for _ in 0..HB_CYCLES {
            led_pwm::set_raw(true);
            block_for(Duration::from_millis(HB_HALF_MS));
            led_pwm::set_raw(false);
            block_for(Duration::from_millis(HB_HALF_MS));
        }

        let mut syst = unsafe { cortex_m::Peripherals::steal() }.SYST;
        syst.disable_interrupt();
        syst.disable_counter();
        led_pwm::deinit();

        unsafe {
            let vector_table = active_offset as *const u32;
            cortex_m::asm::bootload(vector_table)
        }
    }

    #[cfg(not(feature = "noswap"))]
    {
        use embassy_boot::{BootLoader, State};

        let mut config = config;
        let mut state_word = [0u8; WRITE_SIZE];
        config.state.read(0, &mut state_word).unwrap();
        let current_state = State::from(&state_word[..]);

        if current_state == State::Swap {
            info!("swap in progress");
            let progress = current_progress(&mut config.state);
            let pages = (config.active.capacity() / SWAP_PAGE_SIZE) * 2;
            let is_swapped = progress >= pages;
            debug!("progress={}/{}, swapped={}", progress, pages, is_swapped);

            if !is_swapped {
                led_pwm::set_raw(true);
                block_for(Duration::from_millis(PRE_SWAP_MS));
                led_pwm::set_raw(false);
            } else {
                for _ in 0..PRE_REVERT_COUNT {
                    led_pwm::set_raw(true);
                    block_for(Duration::from_millis(PRE_REVERT_BLINK_MS));
                    led_pwm::set_raw(false);
                    block_for(Duration::from_millis(PRE_REVERT_BLINK_MS));
                }
            }
        } else {
            block_for(Duration::from_millis(HB_HALF_MS));
            for _ in 0..HB_CYCLES {
                led_pwm::set_raw(true);
                block_for(Duration::from_millis(HB_HALF_MS));
                led_pwm::set_raw(false);
                block_for(Duration::from_millis(HB_HALF_MS));
            }
        }

        if current_state == State::Swap {
            let progress = current_progress(&mut config.state);
            let is_swapped = progress >= (config.active.capacity() / SWAP_PAGE_SIZE) * 2;
            if !is_swapped {
                led_pwm::start(SWAP_BREATHE_MS);
            }
        }

        // ── DFU slot diagnostics ──
        // Which physical slot holds what right before the boot decision.
        // After a real forward swap, dfu page 1 must contain the OLD firmware's
        // vector table; raw image or garbage there means the backup phase never
        // physically ran. The post-boot active dump shows what got restored.
        #[cfg(feature = "defmt")]
        {
            dump_words("pre state", &mut config.state, 0);
            dump_words("pre active", &mut config.active, 0);
            dump_words("pre dfu", &mut config.dfu, 0);
            #[cfg(feature = "dfu_ext")]
            dump_words("pre dfu+64K", &mut config.dfu, SWAP_PAGE_SIZE as u32);
        }

        let mut page = [0u8; PAGE_SIZE];
        let mut bl = BootLoader::new(config);
        let state = bl.prepare_boot(&mut page).unwrap_or(State::Boot);

        #[cfg(feature = "defmt")]
        {
            debug!(
                "prepare_boot => {}",
                match state {
                    State::Boot => "Boot",
                    State::Swap => "Swap",
                    State::Revert => "Revert",
                    State::DfuDetach => "DfuDetach",
                }
            );
            flash_mutex
                .lock(|c| dump_words("post active", &mut *c.borrow_mut(), active_offset as u32));
        }

        led_pwm::stop();

        if state == State::Swap {
            for _ in 0..POST_SWAP_COUNT {
                led_pwm::set_raw(true);
                block_for(Duration::from_millis(PRE_REVERT_BLINK_MS));
                led_pwm::set_raw(false);
                block_for(Duration::from_millis(PRE_REVERT_BLINK_MS));
            }
        }

        // Disable SysTick and PWM before handing over to firmware
        let mut syst = unsafe { cortex_m::Peripherals::steal() }.SYST;
        syst.disable_interrupt();
        syst.disable_counter();
        led_pwm::deinit();

        unsafe {
            let vector_table = active_offset as *const u32;
            cortex_m::asm::bootload(vector_table)
        }
    }
}

/// DFU slot diagnostics: log the 16 bytes at `off` as hex words.
#[cfg(feature = "defmt")]
fn dump_words<F: embedded_storage::nor_flash::ReadNorFlash>(label: &str, flash: &mut F, off: u32) {
    let mut b = [0u8; 16];
    let _ = flash.read(off, &mut b);
    let w: [u32; 4] =
        core::array::from_fn(|i| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap()));
    debug!(
        "  {} @0x{:06x}: {:08x} {:08x} {:08x} {:08x}",
        label, off, w[0], w[1], w[2], w[3]
    );
}
