use core::cell::{Cell, RefCell};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::blocking_mutex::Mutex;

static MS: Mutex<CriticalSectionRawMutex, Cell<u32>> = Mutex::new(Cell::new(0));
static PERIOD: Mutex<CriticalSectionRawMutex, Cell<u32>> = Mutex::new(Cell::new(0));

// ── nRF52840 ──
#[cfg(feature = "nrf52840")]
use embassy_nrf::pwm::{DutyCycle, SimplePwm};
#[cfg(feature = "nrf52840")]
type PwmDev = SimplePwm<'static>;

#[cfg(feature = "nrf52840")]
static PWM: Mutex<CriticalSectionRawMutex, RefCell<Option<PwmDev>>> = Mutex::new(RefCell::new(None));

#[cfg(feature = "nrf52840")]
fn set_hw_duty(duty: u16) {
    PWM.lock(|c| {
        if let Some(ref mut pwm) = *c.borrow_mut() {
            pwm.set_duty(0, DutyCycle::inverted(duty));
        }
    });
}

// ── RP2040 ──
#[cfg(feature = "rp2040")]
use embassy_rp::pwm::{Pwm, SetDutyCycle};
#[cfg(feature = "rp2040")]
type PwmDev = Pwm<'static>;

#[cfg(feature = "rp2040")]
static PWM: Mutex<CriticalSectionRawMutex, RefCell<Option<PwmDev>>> = Mutex::new(RefCell::new(None));

#[cfg(feature = "rp2040")]
fn set_hw_duty(duty: u16) {
    PWM.lock(|c| {
        if let Some(ref mut pwm) = *c.borrow_mut() {
            let _ = pwm.set_duty_cycle(duty);
        }
    });
}

// ── Public API ──

pub fn init(dev: PwmDev) {
    PWM.lock(|c| *c.borrow_mut() = Some(dev));
}

/// Called each millisecond from SysTick
pub fn tick() {
    let ms = MS.lock(|m| {
        let v = m.get();
        m.set(v + 1);
        v
    });
    let period = PERIOD.lock(|p| p.get());
    if period == 0 {
        return;
    }
    let half = period / 2;
    let cycle = ms % period;
    let t = if cycle <= half { cycle } else { period - cycle };
    let duty = ((t as u64) * 255 / half as u64) as u16;
    set_hw_duty(duty);
}

pub fn start(period_ms: u32) {
    MS.lock(|m| m.set(0));
    PERIOD.lock(|p| p.set(period_ms));
}

pub fn stop() {
    set_hw_duty(0);
    PERIOD.lock(|p| p.set(0));
}

/// Hard on/off for blink codes
pub fn set_raw(on: bool) {
    set_hw_duty(if on { 255 } else { 0 });
}

/// Set PWM duty cycle directly (0-255).
/// Use this from a main loop instead of relying on SysTick.
#[cfg(feature = "nrf52840")]
pub fn set_duty(duty: u8) {
    set_hw_duty(duty as u16);
}

/// Release the PWM peripheral so the firmware can claim it.
pub fn deinit() {
    PWM.lock(|c| { c.borrow_mut().take(); });
}
