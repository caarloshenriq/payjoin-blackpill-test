//! Absolute minimal firmware: blink the onboard LED (PC13) forever, using
//! only raw register writes -- no HAL, no USB, no payjoin, nothing.
//!
//! Purpose: isolate whether the current flashing trouble is about the
//! board/flash process itself, or something specific to the HAL/USB code
//! in usb-echo. If this blinks: board + DFU + basic code execution are
//! all fine, and the problem is somewhere in usb-echo's HAL/USB setup. If
//! this does NOT blink: the problem is upstream of any application code
//! (flashing process, board, or toolchain), not something introduced by
//! adding stm32f4xx-hal/usb-device.
//!
//! PC13 is active-low on the Black Pill: writing 0 to its output data bit
//! turns the LED ON, writing 1 turns it OFF.

#![no_std]
#![no_main]

use cortex_m_rt::entry;
use panic_halt as _;

const RCC_AHB1ENR: *mut u32 = 0x4002_3830 as *mut u32;
const GPIOC_MODER: *mut u32 = 0x4002_0800 as *mut u32;
const GPIOC_ODR: *mut u32 = 0x4002_0814 as *mut u32;

const GPIOCEN: u32 = 1 << 2;
const PIN13_OUTPUT: u32 = 0b01 << (13 * 2);
const PIN13_MODER_MASK: u32 = 0b11 << (13 * 2);
const PIN13_BIT: u32 = 1 << 13;

#[entry]
fn main() -> ! {
    unsafe {
        // Enable the GPIOC peripheral clock.
        core::ptr::write_volatile(RCC_AHB1ENR, core::ptr::read_volatile(RCC_AHB1ENR) | GPIOCEN);

        // Set PC13 to general-purpose output mode.
        let moder = core::ptr::read_volatile(GPIOC_MODER);
        core::ptr::write_volatile(GPIOC_MODER, (moder & !PIN13_MODER_MASK) | PIN13_OUTPUT);
    }

    loop {
        unsafe {
            // LED on (active-low: clear the bit).
            let odr = core::ptr::read_volatile(GPIOC_ODR);
            core::ptr::write_volatile(GPIOC_ODR, odr & !PIN13_BIT);
        }
        delay(2_000_000);

        unsafe {
            // LED off (active-low: set the bit).
            let odr = core::ptr::read_volatile(GPIOC_ODR);
            core::ptr::write_volatile(GPIOC_ODR, odr | PIN13_BIT);
        }
        delay(2_000_000);
    }
}

/// Crude busy-wait delay -- no timer setup, just spending cycles. Not
/// calibrated to a specific duration; the point is "visibly blinking",
/// not a precise interval.
fn delay(cycles: u32) {
    for _ in 0..cycles {
        cortex_m::asm::nop();
    }
}
