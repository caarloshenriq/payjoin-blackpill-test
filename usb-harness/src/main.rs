//! Stage B: wires real USB CDC ACM into harness_device's receiver role.
//! USB bring-up here is copy-identical to usb-echo (confirmed working on
//! real hardware: enumerates, /dev/ttyACM0, echoes bytes).
//!
//! Confirmed working end to end on real hardware (WeAct STM32F411CEU6
//! Black Pill) against sender-sim: full v1 BIP78 round trip, receiver
//! role, real payjoin crypto, real USB CDC transport. See
//! harness-device's `run_receiver` for the protocol logic itself (this
//! file used to have that logic inlined with checkpoint instrumentation
//! for debugging -- removed now that it's confirmed working; call the
//! real library function instead of duplicating its logic here).
//!
//! Two real bugs were found and fixed getting here (both now live in
//! harness-device/harness-host, not here):
//!   - run_receiver was sending raw serialized PSBT bytes, but
//!     process_response (sender side) expects base64-encoded text.
//!   - The mpsc-channel-based host/device split needs a command
//!     translation layer (OutRequest -> OriginalPsbt on the way to the
//!     receiver, SignedPsbt -> InResponse on the way to the sender) --
//!     harness-host's run_v1_roundtrip already does this for two real
//!     boards; sender-sim (and harness-device's own
//!     v1_round_trip_over_real_transport test) needed the same fix.
//!
//! One bug is specific to this firmware and stays fixed here:
//!   - `send_frame` only queues bytes into usbd-serial's internal
//!     buffer; it doesn't guarantee they've gone out over the wire yet.
//!     For a polled (non-interrupt) USB stack, the real IN-endpoint
//!     transfer to the host completes across subsequent poll() calls.
//!     Stopping polling immediately after a "successful" send_frame call
//!     silently lost the response -- the device reported success but the
//!     host never received anything. Fixed by having halt_success keep
//!     polling forever instead of just looping on nop().

#![no_std]
#![no_main]

extern crate alloc;

use bitcoin::hashes::Hash;
use cortex_m_rt::entry;
use harness_device::{Transport, run_receiver};
use linked_list_allocator::LockedHeap;
use panic_halt as _;
use stm32f4xx_hal::{
    gpio::alt::otg_fs::{Dm, Dp},
    otg_fs::{USB, UsbBus, UsbBusType},
    pac,
    prelude::*,
};
use usb_device::UsbError;
use usb_device::prelude::*;
use usbd_serial::SerialPort;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

// 16 KiB was confirmed sufficient for a full v1 receiver round trip
// (BIP78 validation + PSBT parsing) against the fixture PSBT. Revisit if
// a larger real-world PSBT hard-faults.
static mut HEAP: [u8; 16 * 1024] = [0; 16 * 1024];

const RCC_AHB1ENR: *mut u32 = 0x4002_3830 as *mut u32;
const GPIOC_MODER: *mut u32 = 0x4002_0800 as *mut u32;
const GPIOC_ODR: *mut u32 = 0x4002_0814 as *mut u32;
const GPIOCEN: u32 = 1 << 2;
const PIN13_OUTPUT: u32 = 0b01 << (13 * 2);
const PIN13_MODER_MASK: u32 = 0b11 << (13 * 2);
const PIN13_BIT: u32 = 1 << 13;

fn init_led() {
    unsafe {
        core::ptr::write_volatile(RCC_AHB1ENR, core::ptr::read_volatile(RCC_AHB1ENR) | GPIOCEN);
        let moder = core::ptr::read_volatile(GPIOC_MODER);
        core::ptr::write_volatile(GPIOC_MODER, (moder & !PIN13_MODER_MASK) | PIN13_OUTPUT);
    }
}

fn led_on() {
    unsafe {
        let odr = core::ptr::read_volatile(GPIOC_ODR);
        core::ptr::write_volatile(GPIOC_ODR, odr & !PIN13_BIT); // active-low
    }
}

fn delay(cycles: u32) {
    for _ in 0..cycles {
        cortex_m::asm::nop();
    }
}

/// Adapts a polled usb-device + usbd-serial pair to harness_device's
/// Transport trait. Both send and recv drive the USB poll loop
/// themselves, since nothing else in this firmware services the USB
/// peripheral in the background (no interrupt handler set up).
struct UsbTransport<'a> {
    usb_dev: UsbDevice<'a, UsbBusType>,
    serial: SerialPort<'a, UsbBusType>,
}

impl<'a> Transport for UsbTransport<'a> {
    type Error = ();

    fn send(&mut self, bytes: &[u8]) -> Result<(), ()> {
        let mut offset = 0;
        while offset < bytes.len() {
            self.usb_dev.poll(&mut [&mut self.serial]);
            match self.serial.write(&bytes[offset..]) {
                Ok(n) if n > 0 => offset += n,
                Ok(_) => {}
                Err(UsbError::WouldBlock) => {}
                Err(_) => return Err(()),
            }
        }
        Ok(())
    }

    fn recv(&mut self, buf: &mut [u8]) -> Result<usize, ()> {
        self.usb_dev.poll(&mut [&mut self.serial]);
        match self.serial.read(buf) {
            Ok(n) => Ok(n),
            Err(UsbError::WouldBlock) => Ok(0),
            Err(_) => Err(()),
        }
    }
}

/// Success: solid LED on, forever. Keeps servicing the USB peripheral
/// (see module docs for why that matters -- the last chunk of the
/// response can otherwise get stuck in the device-side buffer).
fn halt_success(transport: &mut UsbTransport) -> ! {
    led_on();
    loop {
        transport.usb_dev.poll(&mut [&mut transport.serial]);
    }
}

/// Failure: blinks continuously, forever -- gives time to physically
/// check the board before it would stop, unlike a fixed short burst.
fn halt_failure() -> ! {
    loop {
        led_on();
        delay(1_500_000);
        unsafe {
            let odr = core::ptr::read_volatile(GPIOC_ODR);
            core::ptr::write_volatile(GPIOC_ODR, odr | PIN13_BIT);
        }
        delay(1_500_000);
    }
}

#[entry]
fn main() -> ! {
    init_led();

    unsafe {
        ALLOCATOR
            .lock()
            .init(core::ptr::addr_of_mut!(HEAP) as *mut u8, 16 * 1024);
    }

    let dp = pac::Peripherals::take().unwrap();

    let rcc = dp.RCC.constrain();
    let clocks = rcc
        .cfgr
        .use_hse(25.MHz())
        .sysclk(96.MHz())
        .require_pll48clk()
        .freeze();

    let gpioa = dp.GPIOA.split();
    let usb = USB {
        usb_global: dp.OTG_FS_GLOBAL,
        usb_device: dp.OTG_FS_DEVICE,
        usb_pwrclk: dp.OTG_FS_PWRCLK,
        pin_dm: Dm::PA11(gpioa.pa11.into_alternate()),
        pin_dp: Dp::PA12(gpioa.pa12.into_alternate()),
        hclk: clocks.hclk(),
    };

    static mut EP_MEMORY: [u32; 1024] = [0; 1024];
    let usb_bus = UsbBus::new(usb, unsafe { &mut *core::ptr::addr_of_mut!(EP_MEMORY) });

    let serial = SerialPort::new(&usb_bus);
    let usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27dd))
        .strings(&[StringDescriptors::default()
            .manufacturer("payjoin-blackpill-test")
            .product("harness-device receiver")
            .serial_number("0001")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    let mut transport = UsbTransport { usb_dev, serial };

    // Wait for the host to actually finish USB enumeration/configuration
    // before handing off to run_receiver -- otherwise the first
    // recv_frame call spins against an interface that isn't set up yet.
    while transport.usb_dev.state() != UsbDeviceState::Configured {
        transport.usb_dev.poll(&mut [&mut transport.serial]);
    }

    let receiver_script =
        bitcoin::ScriptBuf::new_p2wpkh(&bitcoin::WPubkeyHash::from_byte_array([0x22; 20]));

    let result = run_receiver(&mut transport, "", |script: &bitcoin::Script| {
        script == receiver_script.as_script()
    });

    match result {
        Ok(_) => halt_success(&mut transport),
        Err(_) => halt_failure(),
    }
}
