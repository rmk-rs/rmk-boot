use embassy_boot::{BootLoader, BootLoaderConfig, State};
use embassy_nrf::nvmc::Nvmc;
use embassy_nrf::pwm::{Prescaler, SimpleConfig, SimplePwm};

use super::*;

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

    let flash = Nvmc::new(p.NVMC);
    let flash_mutex = Mutex::new(RefCell::new(flash));

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
        unsafe { core::ptr::write_volatile(DBL_MEM, 0) };
        let fm: &'static _ = unsafe { &*(&flash_mutex as *const _) };
        crate::dfu::run_dfu_usb(fm);
    } else if reset_pin {
        unsafe { core::ptr::write_volatile(DBL_MEM, DBL_MAGIC) };
        led_pwm::set_raw(false);
        block_for(Duration::from_millis(DTAP_SIGNAL_MS));
        led_pwm::set_raw(true);
    } else if magic == DBL_MAGIC {
        unsafe { core::ptr::write_volatile(DBL_MEM, 0) };
    }

    // Phase 2 — normal boot flow
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

    let mut page = [0u8; PAGE_SIZE];
    let mut bl = BootLoader::new(config);
    let state = bl.prepare_boot(&mut page).unwrap_or(State::Boot);

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
