use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::pwm::{Config as PwmConfig, Pwm};

use super::*;
#[cfg(feature = "dfu_ext")]
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

#[cfg(feature = "rp2040-2mb")]
const FLASH_SIZE: usize = 2 * 1024 * 1024;
#[cfg(feature = "rp2040-4mb")]
const FLASH_SIZE: usize = 4 * 1024 * 1024;
#[cfg(feature = "rp2040-8mb")]
const FLASH_SIZE: usize = 8 * 1024 * 1024;
#[cfg(feature = "rp2040-16mb")]
const FLASH_SIZE: usize = 16 * 1024 * 1024;

pub fn run() -> ! {
    let _p = embassy_rp::init(Default::default());
    let p = unsafe { embassy_rp::Peripherals::steal() };

    // ── PWM LED ──
    let mut cfg = PwmConfig::default();
    cfg.top = 255;
    cfg.enable = true;
    // For different LED pin change p.PIN_25 and PWM_SLICE0 below
    // PIN must match the PWMSLICE and new_output_x, for PWM slice to PIN mapping see:
    // https://rp2040.implrust.com/pwm/pwm-in-rp2040.html#mapping-of-pwm-channels-to-gpio-pins
    // led_pwm::init(Pwm::new_output_a(p.PWM_SLICE0, p.PIN_16, cfg));
    led_pwm::init(Pwm::new_output_b(p.PWM_SLICE4, p.PIN_25, cfg));

    // ── SysTick ──
    let mut syst = unsafe { cortex_m::Peripherals::steal() }.SYST;
    syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
    syst.set_reload(embassy_rp::clocks::clk_sys_freq() / 1000 - 1);
    syst.enable_counter();
    syst.enable_interrupt();

    let flash = Flash::<_, Blocking, FLASH_SIZE>::new_blocking(p.FLASH);
    let flash_mutex = Mutex::new(RefCell::new(flash));

    #[cfg(not(feature = "dfu_ext"))]
    let config = embassy_boot_rp::BootLoaderConfig::from_linkerfile_blocking(
        &flash_mutex, &flash_mutex, &flash_mutex,
    );
    #[cfg(not(feature = "dfu_ext"))]
    let active_offset = config.active.offset();

    #[cfg(feature = "dfu_ext")]
    let ext_mutex: Mutex<CriticalSectionRawMutex, RefCell<_>> = {
        use embassy_rp::gpio::{Level, Output};
        use embassy_rp::spi::{Config as SpiConfig, Spi};

        // Default SPI pins for external flash — change if your board is wired
        // differently.
let mut spi_cfg = SpiConfig::default();
        spi_cfg.frequency = 32_000_000;
        let spi_bus = Spi::new_blocking(p.SPI0, p.PIN_18, p.PIN_19, p.PIN_16, spi_cfg);
        let cs = Output::new(p.PIN_17, Level::High);
        let ext_flash = crate::driver::w25q::W25qNorFlash::<_, _, { 64 * 1024 }>::new(
            spi_bus, cs, EXT_FLASH_SIZE,
        );
        Mutex::new(RefCell::new(ext_flash))
    };
    #[cfg(feature = "dfu_ext")]
    let config = {
        // Get active + state partitions from linker symbols
        let internal_cfg = embassy_boot_rp::BootLoaderConfig::from_linkerfile_blocking(
            &flash_mutex, &flash_mutex, &flash_mutex,
        );

        embassy_boot::BootLoaderConfig {
            active: internal_cfg.active,
            dfu: BlockingPartition::new(&ext_mutex, 0, EXT_FLASH_SIZE),
            state: internal_cfg.state,
        }
    };
    #[cfg(feature = "dfu_ext")]
    let active_offset = config.active.offset();

    #[cfg(feature = "noswap")]
    {
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
            let vector_table =
                (embassy_rp::flash::FLASH_BASE as u32 + active_offset) as *const u32;
            cortex_m::asm::bootload(vector_table)
        }
    }

    #[cfg(not(feature = "noswap"))]
    {
        #[cfg(not(feature = "dfu_ext"))]
        use embassy_boot_rp::{BootLoader, State};
        #[cfg(feature = "dfu_ext")]
        use embassy_boot::{BootLoader, State};
        let mut config = config;

        let mut state_word = [0u8; WRITE_SIZE];
        config.state.read(0, &mut state_word).unwrap();
        let current_state = State::from(&state_word[..]);

        if current_state == State::Swap {
            let progress = current_progress(&mut config.state);
            let is_swapped = progress >= (config.active.capacity() / SWAP_PAGE_SIZE) * 2;

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

        #[cfg(not(feature = "dfu_ext"))]
        let state = {
            let bl: BootLoader = BootLoader::prepare(config);
            bl.state
        };
        #[cfg(feature = "dfu_ext")]
        let state = {
            let mut page = [0u8; PAGE_SIZE];
            let mut bl = BootLoader::new(config);
            bl.prepare_boot(&mut page).unwrap_or(State::Boot)
        };

        led_pwm::stop();

        if state == State::Swap {
            for _ in 0..POST_SWAP_COUNT {
                led_pwm::set_raw(true);
                block_for(Duration::from_millis(PRE_REVERT_BLINK_MS));
                led_pwm::set_raw(false);
                block_for(Duration::from_millis(PRE_REVERT_BLINK_MS));
            }
        }

        let mut syst = unsafe { cortex_m::Peripherals::steal() }.SYST;
        syst.disable_interrupt();
        syst.disable_counter();
        led_pwm::deinit();

        unsafe {
            let vector_table =
                (embassy_rp::flash::FLASH_BASE as u32 + active_offset) as *const u32;
            cortex_m::asm::bootload(vector_table)
        }
    }
}
