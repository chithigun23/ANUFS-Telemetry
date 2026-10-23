//! Telemetry firmware entry point.
//!
//! This is a skeleton that proves the crates link for the STM32H523 and holds
//! the hardware adapters. The RTIC application that wires the tasks together
//! (see `../README.md`) is written during bench bring-up, because clock
//! configuration, interrupt priorities and DMA channel choices need the real
//! board to verify.

#![no_std]
#![no_main]

use panic_halt as _;

#[allow(dead_code)]
mod sd;

#[cortex_m_rt::entry]
fn main() -> ! {
    // Default clocks (internal oscillator). The 24 MHz crystal and PLL are
    // configured at bring-up.
    let _peripherals = embassy_stm32::init(Default::default());
    loop {
        cortex_m::asm::wfi();
    }
}
