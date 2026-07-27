//! Sender-role counterpart to usb-harness. Same USB bring-up (confirmed
//! working: usb-echo, then usb-harness's receiver role, both on this
//! exact board/toolchain) -- only difference is calling
//! harness_device::run_sender instead of run_receiver.
//!
//! Exists because we only have one physical board right now. Rather than
//! wait for a second board to test the sender role in hardware, this
//! runs the sender role on the *same* board, tested against
//! receiver-sim (a host-side tool playing the receiver, mirroring how
//! sender-sim plays the sender against this board's usb-harness). Not a
//! simultaneous two-board test, but it is real hardware exercising the
//! real sender-side payjoin logic over a real USB link -- receiver-sim
//! plays the missing side, the same way sender-sim did for the receiver
//! role.
//!
//! Uses the same fixture (harness_device::original_psbt_fixture) as
//! sender-sim did, so receiver-sim on the other end can use the exact
//! same fixture to validate against.
//!
//! NOTE: different USB VID/PID (0x27de instead of usb-harness's 0x27dd)
//! and product string ("harness-device sender"), so it's distinguishable
//! from the receiver firmware in `lsusb` if both ever get flashed to
//! boards on the same machine.

#![no_std]
#![no_main]

extern crate alloc;

use bitcoin::Amount;
use cortex_m_rt::entry;
use harness_device::{Transport, original_psbt_fixture, run_sender};
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

/// Success: solid LED on, forever, keeps servicing the USB peripheral --
/// same reasoning as usb-harness's halt_success: the last chunk of
/// whatever was still in flight when run_sender returned needs the poll
/// loop to keep running to actually clear the endpoint buffer.
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
    let usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27de))
        .strings(&[StringDescriptors::default()
            .manufacturer("payjoin-blackpill-test")
            .product("harness-device sender")
            .serial_number("0002")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    let mut transport = UsbTransport { usb_dev, serial };

    while transport.usb_dev.state() != UsbDeviceState::Configured {
        transport.usb_dev.poll(&mut [&mut transport.serial]);
    }

    // Wait for the host to actually open the port (asserts DTR) before
    // sending anything. Without this, the device can start writing its
    // OutRequest as soon as USB enumerates -- independent of whether the
    // host-side application has opened the port yet. The OS driver still
    // accepts the bulk transfer at the kernel level even with no reader,
    // but the first byte(s) can get lost/flushed once the host
    // application does finally open() the port. Only matters for the
    // sender role, since it initiates the write; the receiver role
    // (usb-harness) never had this problem because the host always
    // writes first there.
    while !transport.serial.dtr() {
        transport.usb_dev.poll(&mut [&mut transport.serial]);
    }

    let (original_psbt, receiver_address) = original_psbt_fixture();

    let result = run_sender(
        &mut transport,
        original_psbt,
        "https://example.com/",
        &receiver_address,
        Amount::from_sat(182),
    );

    match result {
        Ok(_) => halt_success(&mut transport),
        Err(_) => halt_failure(),
    }
}
