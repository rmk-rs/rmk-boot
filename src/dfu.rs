use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::blocking_mutex::Mutex;
use embassy_nrf::nvmc::Nvmc;

// ---------------------------------------------------------------------------
// Interrupt binding – nRF52
// ---------------------------------------------------------------------------
#[cfg(feature = "nrf528xx")]
embassy_nrf::bind_interrupts! {
    pub(crate) struct DfuIrqs {
        USBD => embassy_nrf::usb::InterruptHandler<embassy_nrf::peripherals::USBD>;
    }
}

// ---------------------------------------------------------------------------
// Waker helpers
// ---------------------------------------------------------------------------
fn noop_raw_waker() -> RawWaker {
    fn clone(_: *const ()) -> RawWaker {
        noop_raw_waker()
    }
    fn wake(_: *const ()) {}
    fn wake_by_ref(_: *const ()) {}
    fn drop(_: *const ()) {}
    const VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake_by_ref, drop);
    RawWaker::new(core::ptr::null(), &VTABLE)
}

/// Poll a future for at most `ms` milliseconds. Returns `true` if it completed.
pub fn poll_for<F: Future>(mut fut: Pin<&mut F>, cx: &mut Context, ms: u64) -> bool {
    let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_millis(ms);
    loop {
        match fut.as_mut().poll(cx) {
            Poll::Ready(_) => return true,
            Poll::Pending => {}
        }
        if embassy_time::Instant::now() >= deadline {
            return false;
        }
    }
}

/// Run USB DFU with breathing LED – never returns.
/// The LED is pulsed from inside this poll loop, so no interrupt (SysTick)
/// interferes with USB during NVMC page erases.
pub fn run_dfu_nrf<D: embassy_usb::driver::Driver<'static>>(
    usb_dev: &mut embassy_usb::UsbDevice<'static, D>,
    period_ms: u32,
) -> ! {
    let waker = unsafe { Waker::from_raw(noop_raw_waker()) };
    let mut cx = Context::from_waker(&waker);
    unsafe { cortex_m::interrupt::enable() }
    let mut usb_fut = core::pin::pin!(usb_dev.run());

    let half = period_ms / 2;
    let mut t: u32 = 0;

    loop {
        // Poll USB for 1 ms
        poll_for(usb_fut.as_mut(), &mut cx, 1);

        // Breathe
        t = (t + 1) % period_ms;
        let pos = if t <= half { t } else { period_ms - t };
        let duty = ((pos as u64) * 255 / half as u64) as u8;
        crate::led_pwm::set_duty(duty);
    }
}

/// Build and run the full USB DFU stack – never returns.
///
/// Called from main.
pub fn run_dfu_usb(
    flash_mutex: &'static Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>,
) -> ! {
    use embassy_boot::{BlockingFirmwareUpdater, FirmwareUpdaterConfig};
    use embassy_usb::class::dfu::consts::DfuAttributes;
    use embassy_usb_dfu::{self as dfu, ResetImmediate};

    let p = unsafe { embassy_nrf::Peripherals::steal() };

    // ── USB driver (nRF USBD peripheral + VBUS detection) ──
    let vbus_obj = embassy_nrf::usb::vbus_detect::SoftwareVbusDetect::new(true, true);
    let vbus: &'static _ = unsafe { &*(&vbus_obj as *const _) };
    let driver = embassy_nrf::usb::Driver::new(p.USBD, DfuIrqs, vbus);

    // ── USB device descriptor ──
    let mut usb_config = embassy_usb::Config::new(0x1209, 0x0001);
    usb_config.manufacturer = Some("rmk-boot");
    usb_config.product = Some("nRF52840 DFU");
    usb_config.serial_number = Some("123456");
    usb_config.max_power = 100;
    usb_config.composite_with_iads = false;
    usb_config.device_class = 0xFE;
    usb_config.device_sub_class = 0x01;
    usb_config.device_protocol = 0x01;

    // ── Static buffers for USB control transfers ──
    static mut CFG: [u8; 256] = [0; 256];
    static mut BOS: [u8; 128] = [0; 128];
    static mut MSOS: [u8; 128] = [0; 128];
    static mut CTL: [u8; 2048] = [0; 2048];
    static mut AL: [u8; 4] = [0; 4];

    // ── Build USB device ──
    let mut builder = embassy_usb::Builder::new(
        driver,
        usb_config,
        unsafe { &mut *core::ptr::addr_of_mut!(CFG) },
        unsafe { &mut *core::ptr::addr_of_mut!(BOS) },
        unsafe { &mut *core::ptr::addr_of_mut!(MSOS) },
        unsafe { &mut *core::ptr::addr_of_mut!(CTL) },
    );

    // ── Firmware-updater over the DFU partition (linker-defined) ──
    let uc = FirmwareUpdaterConfig::from_linkerfile_blocking(flash_mutex, flash_mutex);
    let upd = BlockingFirmwareUpdater::new(uc, unsafe { &mut *core::ptr::addr_of_mut!(AL) });

    // ── DFU class state machine (USB DFU interface) ──
    const BS: usize = 2048;
    let mut s = dfu::new_state::<_, _, ResetImmediate, BS>(
        upd,
        DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
        ResetImmediate,
    );
    let s: &'static mut _ = unsafe { &mut *(&mut s as *mut _) };
    dfu::usb_dfu::<_, _, _, _, BS>(&mut builder, s, |_| {});

    // ── Run USB device; never returns ──
    let mut usb_dev = builder.build();
    run_dfu_nrf(&mut usb_dev, crate::DFU_BREATHE_MS)
}
