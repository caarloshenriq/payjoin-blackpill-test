//! Stage A: bare USB CDC ACM echo. Proves the board enumerates as a
//! serial port and can move bytes, before any harness-device logic gets
//! wired in (usb-harness/, once this is confirmed working).
//!
//! Confirmed working on real hardware (WeAct STM32F411CEU6 Black Pill):
//! enumerates as idVendor=16c0, idProduct=27dd ("harness-device USB
//! CDC"), creates /dev/ttyACM0, echoes bytes back correctly.
//!
//! Flashing notes:
//!   - DFU (dfu-util) was unreliable for this board/cable/adapter
//!     combination in practice -- transfers of any real size tended to
//!     fail partway through with LIBUSB_ERROR_IO/LIBUSB_ERROR_PIPE. SWD
//!     via an ST-Link (probe-rs) was reliable and is what actually got
//!     this flashed and confirmed:
//!       cargo build --release --target thumbv7em-none-eabihf -p usb-echo
//!       probe-rs run --chip STM32F411CEUx target/thumbv7em-none-eabihf/release/usb-echo
//!   - This binary needs its OWN USB-C cable connected from the board to
//!     the PC to be visible as a serial device -- the ST-Link's USB
//!     connection (used only for flashing/debug via SWD) is separate and
//!     does not carry the board's own USB CDC traffic.

#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;
use stm32f4xx_hal::{
    gpio::alt::otg_fs::{Dm, Dp},
    otg_fs::{USB, UsbBus},
    pac,
    prelude::*,
};
use usb_device::prelude::*;
use usbd_serial::SerialPort;

#[entry]
fn main() -> ! {
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

    // usb-device needs the bus allocator and device objects to live for
    // 'static. A static mut with unsafe access is the standard pattern
    // here (single-threaded, no interrupts touching this yet).
    static mut EP_MEMORY: [u32; 1024] = [0; 1024];
    let usb_bus = UsbBus::new(usb, unsafe { &mut *core::ptr::addr_of_mut!(EP_MEMORY) });

    let mut serial = SerialPort::new(&usb_bus);

    // 0x16c0/0x27dd: the shared VID/PID voti.nl provides for open-source
    // USB test devices (used throughout usb-device's own examples). Fine
    // for bring-up; get a real VID/PID before this ships to anyone else.
    let mut usb_dev = UsbDeviceBuilder::new(&usb_bus, UsbVidPid(0x16c0, 0x27dd))
        .strings(&[StringDescriptors::default()
            .manufacturer("payjoin-blackpill-test")
            .product("harness-device USB CDC")
            .serial_number("0001")])
        .unwrap()
        .device_class(usbd_serial::USB_CLASS_CDC)
        .build();

    loop {
        if !usb_dev.poll(&mut [&mut serial]) {
            continue;
        }

        let mut buf = [0u8; 64];
        match serial.read(&mut buf) {
            Ok(n) if n > 0 => {
                // Echo back exactly what we got. Best-effort write: if
                // the host isn't ready yet, drop it rather than block --
                // fine for an echo test, not fine for the real harness
                // Transport (which needs write() to actually retry).
                let _ = serial.write(&buf[..n]);
            }
            _ => {}
        }
    }
}
