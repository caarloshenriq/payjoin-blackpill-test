#![no_std]
#![no_main]

extern crate alloc;

use alloc::format;
use cortex_m::asm::delay;
use cortex_m_rt::entry;
use linked_list_allocator::LockedHeap;
use panic_halt as _;

use payjoin::bitcoin::hashes::{sha256, Hash, HashEngine};
use payjoin::directory::ShortId;

const RCC_BASE: u32 = 0x4002_3800;
const RCC_AHB1ENR: *mut u32 = (RCC_BASE + 0x30) as *mut u32;

const GPIOC_BASE: u32 = 0x4002_0800;
const GPIOC_MODER: *mut u32 = (GPIOC_BASE + 0x00) as *mut u32;
const GPIOC_ODR: *mut u32 = (GPIOC_BASE + 0x14) as *mut u32;

const LED_PIN: u32 = 13;

#[global_allocator]
static ALLOCATOR: LockedHeap = LockedHeap::empty();

static mut HEAP: [u8; 32 * 1024] = [0; 32 * 1024];

fn led_init() {
    unsafe {
        let enr = RCC_AHB1ENR.read_volatile();
        RCC_AHB1ENR.write_volatile(enr | (1 << 2));

        let moder = GPIOC_MODER.read_volatile();
        let moder = moder & !(0b11 << (LED_PIN * 2));
        let moder = moder | (0b01 << (LED_PIN * 2));
        GPIOC_MODER.write_volatile(moder);

        let odr = GPIOC_ODR.read_volatile();
        GPIOC_ODR.write_volatile(odr | (1 << LED_PIN));
    }
}

fn led_set(on: bool) {
    unsafe {
        let odr = GPIOC_ODR.read_volatile();
        if on {
            GPIOC_ODR.write_volatile(odr & !(1 << LED_PIN));
        } else {
            GPIOC_ODR.write_volatile(odr | (1 << LED_PIN));
        }
    }
}

const DELAY_1S: u32 = 16_000_000 / 4;
const DELAY_SHORT: u32 = DELAY_1S / 4;

fn blink(times: u32) {
    for _ in 0..times {
        led_set(true);
        delay(DELAY_SHORT);
        led_set(false);
        delay(DELAY_SHORT);
    }
    delay(DELAY_1S);
}

fn run_payjoin_tests() -> bool {
    let mut engine = sha256::HashEngine::default();
    engine.input(b"payjoin-blackpill-test");
    let hash = sha256::Hash::from_engine(engine);
    let id = ShortId::from(hash);
    let encoded = format!("{}", id);
    let decoded = encoded.parse::<ShortId>().ok();
    let round_trip_ok = decoded.map(|d| id == d).unwrap_or(false);

    let mut engine2 = sha256::HashEngine::default();
    engine2.input(b"ohttp-keys-hash");
    let hash2 = sha256::Hash::from_engine(engine2);
    let mailbox_id = ShortId::from(hash2);
    let mailbox_encoded = format!("{}", mailbox_id);
    let mailbox_ok = !mailbox_encoded.is_empty();

    round_trip_ok && mailbox_ok
}

#[entry]
fn main() -> ! {
    unsafe {
        ALLOCATOR
            .lock()
            .init(core::ptr::addr_of_mut!(HEAP) as *mut u8, 32 * 1024);
    }

    led_init();

    // LED aceso fixo = PASS, piscando 5x = FAIL
    let ok = run_payjoin_tests();

    if ok {
        led_set(true);
        loop {
            cortex_m::asm::wfi();
        }
    } else {
        loop {
            blink(5);
        }
    }
}
