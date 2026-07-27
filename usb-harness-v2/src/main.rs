//! v2 counterpart to usb-harness. Same proven USB bring-up, calling
//! harness_device::run_v2_probe instead of run_receiver.
//!
//! This is the ShortId/mailbox-derivation primitive only -- NOT a live
//! v2 receiver session (see harness_device::run_v2_probe's own docs for
//! why that's not possible on bare-metal). Already proven correct on
//! this exact hardware via payjoin-blackpill-test's original LED-only
//! test; this is the same primitive, but driven over a real USB link
//! against harness-host's --mode v2-probe instead of a bare main() with
//! no host round trip at all.
//!
//! Test with (from payjoin-no-std-harness):
//!   cargo run -p harness-host -- --mode v2-probe \
//!     --device-port /dev/ttyACM0 --seed "payjoin-blackpill-test"

#![no_std]
#![no_main]

extern crate alloc;

use cortex_m_rt::entry;
use harness_device::run_v2_probe;
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

// Smaller than v1's 16 KiB: no PSBT/secp256k1 involved, just SHA256 +
// bech32m encoding. Bump if this ever hard-faults.
static mut HEAP: [u8; 8 * 1024] = [0; 8 * 1024];

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

struct UsbTransport<'a> {
    usb_dev: UsbDevice<'a, UsbBusType>,
    serial: SerialPort<'a, UsbBusType>,
}

impl<'a> harness_device::Transport for UsbTransport<'a> {
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

/// Success: solid LED on, forever, keeps polling -- same reasoning as
/// usb-harness's halt_success (the response can otherwise get stuck in
/// the device-side endpoint buffer).
fn halt_success(transport: &mut UsbTransport) -> ! {
    led_on();
    loop {
        transport.usb_dev.poll(&mut [&mut transport.serial]);
    }
}

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
            .init(core::ptr::addr_of_mut!(HEAP) as *mut u8, 8 * 1024);
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
    let usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27df))
        .strings(&[StringDescriptors::default()
            .manufacturer("payjoin-blackpill-test")
            .product("harness-device v2 probe")
            .serial_number("0003")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    let mut transport = UsbTransport { usb_dev, serial };

    while transport.usb_dev.state() != UsbDeviceState::Configured {
        transport.usb_dev.poll(&mut [&mut transport.serial]);
    }

    // Same DTR wait as usb-harness-sender: this device is passive here
    // (host writes the seed first, matching usb-harness's receiver
    // pattern, not usb-harness-sender's device-initiates pattern), so
    // this isn't strictly needed for a race the way it was for the
    // sender -- kept for consistency and because it's cheap insurance.
    while !transport.serial.dtr() {
        transport.usb_dev.poll(&mut [&mut transport.serial]);
    }

    let result = run_v2_probe(&mut transport);

    match result {
        Ok(_) => halt_success(&mut transport),
        Err(_) => halt_failure(),
    }
}
