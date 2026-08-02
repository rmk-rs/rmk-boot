use embassy_boot_rp::{BootLoader, BootLoaderConfig, State};
use embassy_rp::flash::{Blocking, Flash};
use embassy_rp::pwm::{Config as PwmConfig, Pwm};

use super::*;

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
    // e.g. led_pwm::init(Pwm::new_output_a(p.PWM_SLICE0, p.PIN_16, cfg));
    led_pwm::init(Pwm::new_output_b(p.PWM_SLICE4, p.PIN_25, cfg));

    // ── SysTick ──
    let mut syst = unsafe { cortex_m::Peripherals::steal() }.SYST;
    syst.set_clock_source(cortex_m::peripheral::syst::SystClkSource::Core);
    syst.set_reload(embassy_rp::clocks::clk_sys_freq() / 1000 - 1);
    syst.enable_counter();
    syst.enable_interrupt();

    let flash = Flash::<_, Blocking, FLASH_SIZE>::new_blocking(p.FLASH);
    let flash_mutex = Mutex::new(RefCell::new(flash));

    let mut config =
        BootLoaderConfig::from_linkerfile_blocking(&flash_mutex, &flash_mutex, &flash_mutex);
    let active_offset = config.active.offset();

    let mut state_word = [0u8; WRITE_SIZE];
    config.state.read(0, &mut state_word).unwrap();
    let current_state = State::from(&state_word[..]);

    if current_state == State::Swap {
        let page_count = config.active.capacity() / PAGE_SIZE;
        let progress = current_progress(&mut config.state);
        let is_swapped = progress >= page_count * 2;

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
        let page_count = config.active.capacity() / PAGE_SIZE;
        let progress = current_progress(&mut config.state);
        let is_swapped = progress >= page_count * 2;
        if !is_swapped {
            led_pwm::start(SWAP_BREATHE_MS);
        }
    }

    let bl: BootLoader = BootLoader::prepare(config);

    led_pwm::stop();

    if bl.state == State::Swap {
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
        let vector_table = (embassy_rp::flash::FLASH_BASE as u32 + active_offset) as *const u32;
        cortex_m::asm::bootload(vector_table)
    }
}
