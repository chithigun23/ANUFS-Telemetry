//! Board constants and clock setup shared by the programs in this crate.

use embassy_stm32::time::mhz;
use embassy_stm32::{rcc, Config};
use tel_tlc59108::leds::Bank;

/// Which LED driver pin holds the red die. Unverified: the LED datasheet was
/// not available. If green and red look swapped when the colour test runs, flip
/// this and rebuild.
pub const RED_BANK: Bank = Bank::Pin1;

/// Brightness used by the LED tests (0..=255). Kept modest.
pub const LED_LEVEL: u8 = 80;

/// Try the 24 MHz crystal before the HAL commits to it, so a bad crystal gives a
/// report instead of a hang.
pub fn probe_hse() -> bool {
    use embassy_stm32::pac::RCC;
    RCC.cr().modify(|w| w.set_hseon(true));
    for _ in 0..400 {
        if RCC.cr().read().hserdy() {
            return true;
        }
        cortex_m::asm::delay(10_000);
    }
    RCC.cr().modify(|w| w.set_hseon(false));
    false
}

/// 200 MHz system clock, 80 MHz for FDCAN. Source is the crystal if it started,
/// otherwise the internal oscillator (so the other tests still run).
pub fn clock_config(hse_ok: bool) -> Config {
    let mut c = Config::default();
    if hse_ok {
        c.rcc.hse = Some(rcc::Hse { freq: mhz(24), mode: rcc::HseMode::Oscillator });
    }
    let (source, prediv) =
        if hse_ok { (rcc::PllSource::HSE, rcc::PllPreDiv::DIV3) } else { (rcc::PllSource::HSI, rcc::PllPreDiv::DIV8) };
    // 24 MHz / 3 = 8 MHz (or 64 MHz / 8), x50 = 400 MHz VCO, /2 = 200 MHz, /5 = 80 MHz.
    c.rcc.pll1 = Some(rcc::Pll {
        source,
        prediv,
        mul: rcc::PllMul::MUL50,
        divp: Some(rcc::PllDiv::DIV2),
        divq: Some(rcc::PllDiv::DIV5),
        divr: Some(rcc::PllDiv::DIV2),
    });
    c.rcc.sys = rcc::Sysclk::PLL1_P;
    c.rcc.ahb_pre = rcc::AHBPrescaler::DIV1;
    c.rcc.apb1_pre = rcc::APBPrescaler::DIV2;
    c.rcc.apb2_pre = rcc::APBPrescaler::DIV2;
    c.rcc.apb3_pre = rcc::APBPrescaler::DIV2;
    c.rcc.mux.fdcan12sel = rcc::mux::Fdcansel::PLL1_Q;
    c
}

/// GNSS position fix interval: 100 ms is 10 Hz, the fastest the module allows.
pub const FIX_INTERVAL_MS: u16 = 100;

/// UART baud rate the module is configured for (10 Hz of NMEA needs more than 115200).
pub const GNSS_BAUD: u32 = 460_800;

/// `$PQTMTAR` output rate: 1 = every fix. The rover (U13) sends its heading to
/// the base (U10) over the UART2 link, and U10 is expected to report it on UART1
/// to the MCU. Not yet confirmed on hardware.
pub const TAR_RATE: u8 = 1;

/// CAN1 (MoTeC network) bit rate. Unknown until the MoTeC configuration is
/// supplied; 1 Mbit/s is a placeholder.
pub const CAN1_BITRATE: u32 = 1_000_000;

/// CAN2 (SBG Ellipse) bit rate. 1 Mbit/s is a placeholder: confirm against the
/// Ellipse output configuration.
pub const CAN2_BITRATE: u32 = 1_000_000;

/// IMU output data rate recorded in the session header.
pub const IMU_ODR_HZ: u16 = 416;

/// Firmware version string recorded in the session header (16 bytes, zero padded).
pub const FIRMWARE: [u8; 16] = *b"tel-fw 0.1.0\0\0\0\0";
