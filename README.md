> [!CAUTION]
> This codebase was created with heavy reliance on AI models. This is not a
> demonstration of a good reference implementation for no_std payjoin
> implementations.

# payjoin-blackpill-test — `payjoin` no_std on STM32F411CEU6 (Black Pill)

Compile `payjoin` (no_std, v2 features) on a bare-metal STM32F411CEU6
microcontroller (ARM Cortex-M4F, `thumbv7em-none-eabihf`).

This is a companion test to
[payjoin-pico2-test](https://github.com/benalleng/payjoin-pico2), which tested
the same PR on a Cortex-M33 (RP2350). This repo covers the PR's declared CI
target: `thumbv7em-none-eabihf`.

## What is tested

- `payjoin::directory::ShortId` round-trip (sha256::Hash → ShortId → bech32m → ShortId)
- SHA256 → ShortId mailbox derivation (used by v2 receiver)

The full `receive::v2` receiver state machine is gated on `v2-ohttp` (requires
`std`). A live receiver session is not possible on bare-metal.

## Hardware

- WeAct STM32F411CEU6 Black Pill (or compatible)
- USB-C cable (for power and DFU flashing — no ST-Link required)

## LED output (PC13, active low)

| Pattern             | Meaning                             |
| ------------------- | ----------------------------------- |
| LED on solid        | **PASS** — all tests passed         |
| 5 blinks, repeating | **FAIL** — one or more tests failed |

## Prerequisites

- Nix with flakes enabled
- The `embedded` devShell from
  [rust-payjoin](https://github.com/caarloshenriq/rust-payjoin/tree/feat/payjoin-nostd)
- `dfu-util` (available in the devShell)

## Build

From the workspace root (`../rust-payjoin/`), enter the embedded devShell:

```sh
nix develop .#embedded -c bash
```

Then, from this repo:

```sh
cargo build --release
```

## Flash via DFU (no ST-Link required)

**1. Enter DFU mode** — hold `BOOT0` button, tap `RESET`, then release `BOOT0`.
Confirm the device appears as `0483:df11`:

```sh
lsusb | grep -i dfu
```

**2. Flash (run from inside the embedded devShell):**

```sh
sudo $(which dfu-util) -a 0 -s 0x08000000:leave -D \
  target/thumbv7em-none-eabihf/release/payjoin-blackpill-test
```

> **Note:** DFU flashing stability depends on the USB cable. If the transfer
> fails, retry the command — the device remains in DFU mode after a failed
> attempt. A data-capable USB-C cable is required.

**3. Observe the LED (PC13)**

- LED on solid → **PASS**
- 5 blinks repeating → **FAIL**

## Dependencies

- `payjoin` (git, branch `feat/payjoin-nostd`, no_std, v2)
- `cortex-m`, `cortex-m-rt`
- `panic-halt`
- `linked_list_allocator` (heap for `alloc`)
