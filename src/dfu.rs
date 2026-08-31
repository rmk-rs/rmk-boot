use core::cell::RefCell;
use core::future::Future;
use core::pin::Pin;
use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

use crate::log::info;
use embassy_boot::{BlockingFirmwareUpdater, FirmwareUpdaterConfig};
use embassy_embedded_hal::flash::partition::BlockingPartition;
#[cfg(feature = "dfu_ext")]
use embassy_nrf::gpio::Output;
use embassy_nrf::nvmc::Nvmc;
#[cfg(feature = "dfu_ext")]
use embassy_nrf::spim::Spim;
use embassy_nrf::usb::vbus_detect::SoftwareVbusDetect;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_usb::class::dfu::consts::DfuAttributes;
use embassy_usb_dfu::{self as dfu, ResetImmediate};
use embedded_storage::nor_flash::NorFlash;
use static_cell::StaticCell;

const BS: usize = 2048;
/// nRF52 NVMC write size — embassy-boot's `aligned` buffer requirement
const NRF_WRITE_SIZE: usize = 4;

/// USB product string reported to the host during DFU.
#[cfg(not(feature = "nrf52833"))]
const USB_PRODUCT: &str = "nRF52840 DFU";
#[cfg(feature = "nrf52833")]
const USB_PRODUCT: &str = "nRF52833 DFU";

type NativePartition = BlockingPartition<'static, NoopRawMutex, Nvmc<'static>>;

#[cfg(not(feature = "dfu_ext"))]
static DFU_STATE: StaticCell<
    dfu::State<'static, NativePartition, NativePartition, ResetImmediate, { BS }>,
> = StaticCell::new();

#[cfg(feature = "dfu_ext")]
pub(crate) type ExtFlash =
    crate::driver::w25q::W25qNorFlash<Spim<'static>, Output<'static>, { 64 * 1024 }>;
#[cfg(feature = "dfu_ext")]
type ExtPartition = BlockingPartition<'static, NoopRawMutex, ExtFlash>;
#[cfg(feature = "dfu_ext")]
static EXT_DFU_STATE: StaticCell<
    dfu::State<'static, ExtPartition, NativePartition, ResetImmediate, { BS }>,
> = StaticCell::new();

static VBUS: StaticCell<SoftwareVbusDetect> = StaticCell::new();

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
#[cfg(not(feature = "dfu_ext"))]
pub fn run_dfu_usb(flash_mutex: &'static Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>) -> ! {
    info!("USB DFU active (internal DFU partition)");

    let uc = FirmwareUpdaterConfig::from_linkerfile_blocking(flash_mutex, flash_mutex);

    // noswap never touches the state partition, so a stale Swap marker from an
    // older layout makes write_firmware() refuse downloads (BadState). An
    // erased state page reads as State::Boot — enough to accept downloads.
    #[cfg(feature = "noswap")]
    let uc = {
        let mut uc = uc;
        uc.state
            .erase(0, <Nvmc as NorFlash>::ERASE_SIZE as u32)
            .unwrap();
        uc
    };

    run_dfu_usb_inner(&DFU_STATE, uc)
}

/// Same as [`run_dfu_usb`], but writes the DFU image to an external
/// SPI flash instead of the internal DFU partition.
#[cfg(feature = "dfu_ext")]
pub fn run_dfu_usb_ext(
    flash_mutex: &'static Mutex<NoopRawMutex, RefCell<Nvmc<'static>>>,
    ext_mutex: &'static Mutex<NoopRawMutex, RefCell<ExtFlash>>,
    ext_flash_size: u32,
) -> ! {
    info!(
        "USB DFU active (external SPI flash, size 0x{:x})",
        ext_flash_size
    );

    let dfu_part = BlockingPartition::new(ext_mutex, 0, ext_flash_size);
    let state_part =
        FirmwareUpdaterConfig::from_linkerfile_blocking(flash_mutex, flash_mutex).state;
    let uc = FirmwareUpdaterConfig {
        dfu: dfu_part,
        state: state_part,
    };
    run_dfu_usb_inner(&EXT_DFU_STATE, uc)
}

/// Shared core of the USB DFU stack – never returns.
///
/// The DFU partition type `D` is either the internal (linker-defined)
/// partition or the external SPI flash partition, depending on the
/// `dfu_ext` feature selected by the caller's `state_cell`.
fn run_dfu_usb_inner<D: NorFlash>(
    state_cell: &'static StaticCell<dfu::State<'static, D, NativePartition, ResetImmediate, BS>>,
    uc: FirmwareUpdaterConfig<D, NativePartition>,
) -> ! {
    let p = unsafe { embassy_nrf::Peripherals::steal() };

    // ── USB driver (nRF USBD peripheral + VBUS detection) ──
    let vbus: &'static _ = VBUS.init(SoftwareVbusDetect::new(true, true));
    let driver = embassy_nrf::usb::Driver::new(p.USBD, DfuIrqs, vbus);

    let mut usb_config = embassy_usb::Config::new(0x1209, 0x0001);
    usb_config.manufacturer = Some("rmk-boot");
    usb_config.product = Some(USB_PRODUCT);
    usb_config.serial_number = Some("123456");
    usb_config.max_power = 100;
    usb_config.composite_with_iads = false;
    usb_config.device_class = 0xFE;
    usb_config.device_sub_class = 0x01;
    usb_config.device_protocol = 0x01;

    // ── Static buffers for USB control transfers ──
    static CFG: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS: StaticCell<[u8; 128]> = StaticCell::new();
    static MSOS: StaticCell<[u8; 128]> = StaticCell::new();
    static CTL: StaticCell<[u8; 2048]> = StaticCell::new();
    static AL: StaticCell<[u8; NRF_WRITE_SIZE]> = StaticCell::new();

    // ── Build USB device ──
    let mut builder = embassy_usb::Builder::new(
        driver,
        usb_config,
        CFG.init([0; 256]),
        BOS.init([0; 128]),
        MSOS.init([0; 128]),
        CTL.init([0; 2048]),
    );

    let upd = BlockingFirmwareUpdater::new(uc, AL.init([0; NRF_WRITE_SIZE]));
    let state = dfu::new_state::<D, NativePartition, ResetImmediate, BS>(
        upd,
        DfuAttributes::CAN_DOWNLOAD | DfuAttributes::WILL_DETACH,
        ResetImmediate,
    );
    let s: &'static mut _ = state_cell.init(state);
    dfu::usb_dfu::<_, _, _, _, BS>(&mut builder, s, |_| {});

    // ── Run USB device; never returns ──
    let mut usb_dev = builder.build();
    run_dfu_nrf(&mut usb_dev, crate::DFU_BREATHE_MS)
}
