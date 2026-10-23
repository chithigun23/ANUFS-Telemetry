//! Board bring-up self-test.
//!
//! Flash this once the board is assembled. It checks each feature and device in
//! turn, prints one line per check over RTT (defmt), and shows the result on the
//! five status LEDs (green = every check of that group passed, red = at least
//! one failed, dark = nothing ran).
//!
//! LED mapping: 0 system (clocks, watchdog, buttons), 1 GNSS, 2 SD1, 3 SD2,
//! 4 sensors (IMU, LED drivers, CAN). See `tel-selftest` for the group rules.
//!
//! The pass/fail rules live in the `tel-selftest` crate, where they are unit
//! tested on the PC. This file only talks to the hardware.
//!
//! Safe by default: nothing here drives an output into a load. The SD write test
//! saves the blocks it uses and restores them afterwards.
//!
//! Status: compiled for the STM32H523 but never run on hardware. Every pin
//! assignment comes from the netlist; expect to fix things when the first board
//! is powered.

#![no_std]
#![no_main]
// Each test takes the peripherals it needs by value, so the lists are long by design.
#![allow(clippy::too_many_arguments)]

use board::{clock_config, probe_hse, LED_LEVEL, RED_BANK};
use defmt::info;
use embassy_executor::Spawner;
use embassy_futures::join::join;
use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};
use embassy_stm32::mode::{Async, Blocking};
use embassy_stm32::time::{khz, mhz};
use embassy_stm32::wdg::IndependentWatchdog;
use embassy_stm32::{bind_interrupts, can, exti, i2c, interrupt, peripherals, sdmmc, spi, usart, Peri};
use embassy_time::{with_timeout, Delay, Duration, Instant, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use tel_asm330::{Asm330, Config as ImuConfig, Scaled};
use tel_nmea::{Command, LineAssembler, Message};
use tel_selftest::{
    boot_signature, can_loopback, clock_vs_pps, detail, gnss_fix, gnss_stream, imu_static, led_drivers, pps, readback,
    sd_card, Detail, GnssTally, Group, Outcome, Report,
};
use tel_store::BlockDevice;
use tel_tlc59108::leds::{Bank, Color, ColorMap, StatusLeds};
use tel_tlc59108::Tlc59108;
use {defmt_rtt as _, panic_probe as _};

#[path = "../board.rs"]
#[allow(dead_code)]
mod board;
#[path = "../sd.rs"]
#[allow(dead_code)]
mod sd;

bind_interrupts!(struct Irqs {
    EXTI3 => exti::InterruptHandler<interrupt::typelevel::EXTI3>;
    EXTI9 => exti::InterruptHandler<interrupt::typelevel::EXTI9>;
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
    FDCAN2_IT0 => can::IT0InterruptHandler<peripherals::FDCAN2>;
    FDCAN2_IT1 => can::IT1InterruptHandler<peripherals::FDCAN2>;
    SDMMC1 => sdmmc::InterruptHandler<peripherals::SDMMC1>;
    UART4 => usart::InterruptHandler<peripherals::UART4>;
    GPDMA1_CHANNEL0 => embassy_stm32::dma::InterruptHandler<peripherals::GPDMA1_CH0>;
    GPDMA1_CHANNEL1 => embassy_stm32::dma::InterruptHandler<peripherals::GPDMA1_CH1>;
});

/// Capacity of the report.
const REPORT_LEN: usize = 64;
type Rep = Report<REPORT_LEN>;

/// Blocks used by the SD write test, taken from the end of the card.
const SD_TEST_BLOCKS: usize = 16;

fn rec(r: &mut Rep, group: Group, name: &'static str, res: (Outcome, Detail)) {
    let (outcome, detail) = res;
    let tag = match outcome {
        Outcome::Pass => "PASS",
        Outcome::Fail => "FAIL",
        Outcome::Skip => "SKIP",
        Outcome::Info => "INFO",
    };
    info!("[{}] {} - {}", tag, name, detail.as_str());
    r.push(group, name, outcome, detail);
}

// ------------------------------------------------------------------ clocks and system

fn reset_cause() -> Detail {
    use embassy_stm32::pac::RCC;
    let s = RCC.rsr().read();
    let cause = if s.iwdgrstf() {
        "independent watchdog"
    } else if s.wwdgrstf() {
        "window watchdog"
    } else if s.sftrstf() {
        "software"
    } else if s.borrstf() {
        "power-on or brown-out"
    } else if s.pinrstf() {
        "reset pin"
    } else {
        "unknown"
    };
    RCC.rsr().modify(|w| w.set_rmvf(true));
    detail!("last reset: {cause}")
}

fn system_checks(r: &mut Rep, hse_ok: bool) {
    rec(r, Group::System, "sys.reset", (Outcome::Info, reset_cause()));
    let outcome = if hse_ok { Outcome::Pass } else { Outcome::Fail };
    let text = if hse_ok {
        Detail::text("24 MHz crystal Y1 started; core runs at 200 MHz from the PLL")
    } else {
        Detail::text("crystal Y1 did not start (check Y1, C15, C16); running from the internal oscillator")
    };
    rec(r, Group::System, "sys.hse", (outcome, text));
    rec(r, Group::System, "sys.uid", (Outcome::Info, detail!("device id {}", embassy_stm32::uid::uid_hex())));
}

fn button_checks(
    r: &mut Rep,
    pc0: Peri<'static, peripherals::PC0>,
    pc1: Peri<'static, peripherals::PC1>,
    pc2: Peri<'static, peripherals::PC2>,
    pc3: Peri<'static, peripherals::PC3>,
) {
    let a = Input::new(pc0, Pull::Up);
    let b = Input::new(pc1, Pull::Up);
    let c = Input::new(pc2, Pull::Up);
    let d = Input::new(pc3, Pull::Up);
    let lv = |i: &Input<'_>| u8::from(i.is_high());
    rec(
        r,
        Group::System,
        "sys.switches",
        (
            Outcome::Info,
            detail!(
                "SW1 pins PC0 {} PC1 {}, SW2 pins PC2 {} PC3 {} (1 = open; move the switches and rerun to see them change)",
                lv(&a),
                lv(&b),
                lv(&c),
                lv(&d)
            ),
        ),
    );
}

// ------------------------------------------------------------------ LED drivers

struct Leds {
    bus: i2c::I2c<'static, Blocking, i2c::Master>,
    leds: StatusLeds,
    _reset: Output<'static>,
}

fn i2c_present(bus: &mut i2c::I2c<'static, Blocking, i2c::Master>, addr: u8) -> bool {
    let mut b = [0_u8; 1];
    bus.blocking_write_read(addr, &[0x00], &mut b).is_ok()
}

async fn led_test(
    r: &mut Rep,
    i2c3: Peri<'static, peripherals::I2C3>,
    scl: Peri<'static, peripherals::PA8>,
    sda: Peri<'static, peripherals::PB4>,
    rst: Peri<'static, peripherals::PB5>,
) -> Option<Leds> {
    // The reset pin of both drivers is released by driving it high.
    let reset = Output::new(rst, Level::Low, Speed::Low);
    Timer::after_millis(2).await;
    let mut reset = reset;
    reset.set_high();
    Timer::after_millis(5).await;

    let mut cfg = i2c::Config::default();
    cfg.frequency = khz(100);
    let mut bus = i2c::I2c::new_blocking(i2c3, scl, sda, cfg);
    let u8_ok = i2c_present(&mut bus, 0x40);
    let u9_ok = i2c_present(&mut bus, 0x41);
    rec(r, Group::Sensors, "led.i2c", led_drivers(u8_ok, u9_ok));
    if !(u8_ok && u9_ok) {
        return None;
    }

    let mut leds = StatusLeds::new(ColorMap { red: RED_BANK });
    let mut delay = Delay;
    let res = leds.init(&mut bus, &mut delay).map_err(|e| detail!("init failed: {e:?}"));
    if let Err(d) = res {
        rec(r, Group::Sensors, "led.init", (Outcome::Fail, d));
        return None;
    }
    // Identify which driver pin is which colour, by eye.
    for bank in [Bank::Pin1, Bank::Pin3] {
        match leds.light_bank(&mut bus, bank, LED_LEVEL) {
            Ok(()) => {}
            Err(e) => rec(r, Group::Sensors, "led.bank", (Outcome::Fail, detail!("{bank:?} flush failed: {e:?}"))),
        }
        info!("LEDs: lighting driver bank {} on all five LEDs; note the colour", bank_name(bank));
        // open-load flags with the bank lit
        let chip = Tlc59108::from_pins_masked(if bank == Bank::Pin1 { 0 } else { 1 });
        match chip.error_flags(&mut bus) {
            Ok(flags) => {
                let open = flags & 0x1F;
                rec(
                    r,
                    Group::Sensors,
                    "led.eflag",
                    (
                        Outcome::Info,
                        detail!(
                            "bank {} error flags (LED0..4) = {:05b} (a set bit may mean an open LED; unverified)",
                            bank_name(bank),
                            open
                        ),
                    ),
                );
            }
            Err(e) => rec(r, Group::Sensors, "led.eflag", (Outcome::Fail, detail!("read failed: {e:?}"))),
        }
        Timer::after_millis(1500).await;
    }
    leds.set_all(Color::Amber, LED_LEVEL);
    let _ = leds.flush(&mut bus);
    info!("LEDs: both colours on all five (amber)");
    Timer::after_millis(1200).await;
    for i in 0..5 {
        leds.set_all(Color::Off, 0);
        let _ = leds.set(i, Color::Green, LED_LEVEL);
        let _ = leds.flush(&mut bus);
        info!("LEDs: only LED {} lit (green by assumption)", i);
        Timer::after_millis(400).await;
    }
    leds.set_all(Color::Off, 0);
    let verify = leds.flush(&mut bus).and_then(|()| leds.verify(&mut bus));
    match verify {
        Ok(()) => rec(r, Group::Sensors, "led.verify", (Outcome::Pass, Detail::text("registers read back as written"))),
        Err(e) => rec(r, Group::Sensors, "led.verify", (Outcome::Fail, detail!("readback mismatch: {e:?}"))),
    }
    Some(Leds { bus, leds, _reset: reset })
}

fn bank_name(b: Bank) -> &'static str {
    match b {
        Bank::Pin1 => "U8 (LED pin 1)",
        Bank::Pin3 => "U9 (LED pin 3)",
    }
}

// ------------------------------------------------------------------ IMU

async fn imu_test(
    r: &mut Rep,
    spi1: Peri<'static, peripherals::SPI1>,
    sck: Peri<'static, peripherals::PA5>,
    mosi: Peri<'static, peripherals::PA7>,
    miso: Peri<'static, peripherals::PA6>,
    cs: Peri<'static, peripherals::PA4>,
    int1: Peri<'static, peripherals::PC4>,
    int2: Peri<'static, peripherals::PC5>,
) {
    let mut cfg = spi::Config::default();
    cfg.mode = spi::MODE_3;
    cfg.frequency = mhz(4);
    let bus = spi::Spi::new_blocking(spi1, sck, mosi, miso, cfg);
    let cs = Output::new(cs, Level::High, Speed::VeryHigh);
    let dev = match ExclusiveDevice::new_no_delay(bus, cs) {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut imu = Asm330::new(dev);
    let mut delay = Delay;

    match imu.who_am_i() {
        Ok(id) if id == tel_asm330::WHO_AM_I_VALUE => {
            rec(r, Group::Sensors, "imu.whoami", (Outcome::Pass, detail!("WHO_AM_I = {id:#04x}")));
        }
        Ok(id) => {
            rec(
                r,
                Group::Sensors,
                "imu.whoami",
                (Outcome::Fail, detail!("read {id:#04x}, expected 0x6B (check SPI1 wiring, CS PA4, U3 soldering)")),
            );
            return;
        }
        Err(e) => {
            rec(r, Group::Sensors, "imu.whoami", (Outcome::Fail, detail!("SPI error {e:?}")));
            return;
        }
    }
    if let Err(e) = imu.init(ImuConfig::default(), &mut delay) {
        rec(r, Group::Sensors, "imu.init", (Outcome::Fail, detail!("configuration failed: {e:?}")));
        return;
    }
    rec(
        r,
        Group::Sensors,
        "imu.init",
        (Outcome::Pass, Detail::text("configured +/-16 g, 2000 dps, 416 Hz; registers read back")),
    );

    // INT1 data-ready: flush any pending data, then it must come back high.
    let int1 = Input::new(int1, Pull::None);
    let int2 = Input::new(int2, Pull::None);
    let _ = imu.read_raw();
    let mut seen_high = false;
    let end = Instant::now() + Duration::from_millis(20);
    while Instant::now() < end {
        if int1.is_high() {
            seen_high = true;
            break;
        }
    }
    let int_res = if seen_high {
        (Outcome::Pass, Detail::text("INT1 (PC4) went high on data ready"))
    } else {
        (Outcome::Fail, Detail::text("INT1 (PC4) did not signal data ready within 20 ms (check trace to PC4)"))
    };
    rec(r, Group::Sensors, "imu.int1", int_res);
    rec(
        r,
        Group::Sensors,
        "imu.int2",
        (Outcome::Info, detail!("INT2 (PC5) idle level {} (not used by the firmware yet)", u8::from(int2.is_high()))),
    );

    // Collect samples with the board at rest.
    let mut samples = [Scaled { accel_micro_g: [0; 3], gyro_mdps: [0; 3], temp_centi_c: 0 }; 48];
    let mut n = 0;
    let deadline = Instant::now() + Duration::from_millis(500);
    while n < samples.len() && Instant::now() < deadline {
        match imu.status() {
            Ok(s) if s.accel_ready && s.gyro_ready => {
                if let (Ok(v), Some(slot)) = (imu.read_scaled(), samples.get_mut(n)) {
                    *slot = v;
                    n += 1;
                }
            }
            _ => {}
        }
        Timer::after_micros(500).await;
    }
    let got = samples.get(..n).unwrap_or(&[]);
    rec(r, Group::Sensors, "imu.static", imu_static(got));
}

// ------------------------------------------------------------------ CAN

/// Send frames and read each back; returns (sent, received, mismatched).
async fn loop_frames(bus: &mut can::Can<'_>) -> (u32, u32, u32) {
    let (mut sent, mut got, mut bad) = (0_u32, 0_u32, 0_u32);
    for i in 0_u16..32 {
        let id = 0x100 + i;
        let mut data = [0_u8; 8];
        for (k, b) in data.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31).wrapping_add((k as u8).wrapping_mul(7));
        }
        let Ok(frame) = can::frame::Frame::new_standard(id, &data) else { continue };
        sent += 1;
        bus.write(&frame).await;
        if let Ok(Ok(env)) = with_timeout(Duration::from_millis(50), bus.read()).await {
            got += 1;
            if env.frame.id() != frame.id() || env.frame.data() != frame.data() {
                bad += 1;
            }
        }
    }
    (sent, got, bad)
}

async fn can_internal_test(
    r: &mut Rep,
    fdcan1: &mut Peri<'static, peripherals::FDCAN1>,
    rx1: &mut Peri<'static, peripherals::PB8>,
    tx1: &mut Peri<'static, peripherals::PB7>,
    fdcan2: &mut Peri<'static, peripherals::FDCAN2>,
    rx2: &mut Peri<'static, peripherals::PB12>,
    tx2: &mut Peri<'static, peripherals::PB13>,
) {
    {
        let mut cfg = can::CanConfigurator::new(fdcan1.reborrow(), rx1.reborrow(), tx1.reborrow(), Irqs);
        cfg.set_bitrate(500_000);
        let mut bus = cfg.into_internal_loopback_mode();
        let (s, g, b) = loop_frames(&mut bus).await;
        rec(r, Group::Sensors, "can1.loopback", can_loopback(s, g, b, "FDCAN1 internal loopback"));
    }
    {
        let mut cfg = can::CanConfigurator::new(fdcan2.reborrow(), rx2.reborrow(), tx2.reborrow(), Irqs);
        cfg.set_bitrate(500_000);
        let mut bus = cfg.into_internal_loopback_mode();
        let (s, g, b) = loop_frames(&mut bus).await;
        rec(r, Group::Sensors, "can2.loopback", can_loopback(s, g, b, "FDCAN2 internal loopback"));
    }
}

/// With J4 joined to J5 (CAN_H to CAN_H, CAN_L to CAN_L, and 120 ohm across the
/// pair), a frame from one controller must arrive at the other. This is the only
/// test that exercises the transceivers and the connectors.
async fn can_cross_test(
    r: &mut Rep,
    fdcan1: &mut Peri<'static, peripherals::FDCAN1>,
    rx1: &mut Peri<'static, peripherals::PB8>,
    tx1: &mut Peri<'static, peripherals::PB7>,
    fdcan2: &mut Peri<'static, peripherals::FDCAN2>,
    rx2: &mut Peri<'static, peripherals::PB12>,
    tx2: &mut Peri<'static, peripherals::PB13>,
    flt1: Peri<'static, peripherals::PB6>,
    flt2: Peri<'static, peripherals::PB10>,
    sil: Peri<'static, peripherals::PA10>,
) {
    let flt1 = Input::new(flt1, Pull::Up);
    let flt2 = Input::new(flt2, Pull::Up);
    // Silent control of the second transceiver: low = normal (unverified polarity).
    let _sil = Output::new(sil, Level::Low, Speed::Low);
    Timer::after_millis(5).await;
    rec(
        r,
        Group::Sensors,
        "can.flt",
        (
            Outcome::Info,
            detail!(
                "transceiver fault pins: CAN1 PB6 {}, CAN2 PB10 {} (assumed active low; unverified)",
                u8::from(flt1.is_high()),
                u8::from(flt2.is_high())
            ),
        ),
    );

    let mut c1 = can::CanConfigurator::new(fdcan1.reborrow(), rx1.reborrow(), tx1.reborrow(), Irqs);
    c1.set_bitrate(500_000);
    let mut c2 = can::CanConfigurator::new(fdcan2.reborrow(), rx2.reborrow(), tx2.reborrow(), Irqs);
    c2.set_bitrate(500_000);
    let mut b1 = c1.into_normal_mode();
    let mut b2 = c2.into_normal_mode();

    let Ok(f12) = can::frame::Frame::new_standard(0x321, &[1, 2, 3, 4, 5, 6, 7, 8]) else { return };
    let Ok(f21) = can::frame::Frame::new_standard(0x123, &[8, 7, 6, 5, 4, 3, 2, 1]) else { return };

    let a = with_timeout(Duration::from_millis(100), join(b1.write(&f12), b2.read())).await;
    let ok12 = matches!(&a, Ok((_, Ok(env))) if env.frame.id() == f12.id() && env.frame.data() == f12.data());
    let b = with_timeout(Duration::from_millis(100), join(b2.write(&f21), b1.read())).await;
    let ok21 = matches!(&b, Ok((_, Ok(env))) if env.frame.id() == f21.id() && env.frame.data() == f21.data());

    let res = match (ok12, ok21) {
        (true, true) => {
            (Outcome::Pass, Detail::text("frames crossed both ways between CAN1 and CAN2 through both transceivers"))
        }
        (false, false) => (
            Outcome::Skip,
            Detail::text("no frame crossed: join J4 to J5 (H to H, L to L, 120 ohm across) to test the transceivers"),
        ),
        (true, false) => (
            Outcome::Fail,
            Detail::text("CAN1 to CAN2 works but CAN2 to CAN1 does not (check CAN2 transmit path U7, PB13, PA10)"),
        ),
        (false, true) => (
            Outcome::Fail,
            Detail::text("CAN2 to CAN1 works but CAN1 to CAN2 does not (check CAN1 transmit path U4, PB7)"),
        ),
    };
    rec(r, Group::Sensors, "can.cross", res);
}

// ------------------------------------------------------------------ GNSS

/// What a listening window saw besides the NMEA statistics.
#[derive(Default)]
struct Extra {
    fix_interval_ms: Option<u16>,
    acked: bool,
}

async fn listen(uart: &mut usart::Uart<'static, Async>, window: Duration, t: &mut GnssTally, x: &mut Extra) {
    let end = Instant::now() + window;
    let mut asm = LineAssembler::new();
    let mut buf = [0_u8; 128];
    loop {
        let now = Instant::now();
        if now >= end {
            break;
        }
        match with_timeout(end - now, uart.read_until_idle(&mut buf)).await {
            Ok(Ok(n)) => {
                for &byte in buf.get(..n).unwrap_or(&[]) {
                    match asm.push(byte) {
                        Some(Ok(sentence)) => match tel_nmea::msg::parse(&sentence) {
                            Ok(msg) => {
                                match &msg {
                                    Message::FixRate { interval_ms } => x.fix_interval_ms = Some(*interval_ms),
                                    Message::PairAck { .. } => x.acked = true,
                                    _ => {}
                                }
                                t.feed(&msg);
                            }
                            Err(_) => t.feed_unparsed(),
                        },
                        Some(Err(_)) => t.feed_error(),
                        None => {}
                    }
                }
            }
            Ok(Err(_)) => t.feed_error(),
            Err(_) => break,
        }
    }
}

async fn send(uart: &mut usart::Uart<'static, Async>, cmd: Command<'_>) {
    let mut out = [0_u8; 96];
    if let Ok(n) = cmd.encode(&mut out) {
        let _ = uart.write(out.get(..n).unwrap_or(&[])).await;
    }
}

/// Returns the tally of the best baud rate, for the PPS expectations.
async fn gnss_test(
    r: &mut Rep,
    uart4: Peri<'static, peripherals::UART4>,
    rx: Peri<'static, peripherals::PA1>,
    tx: Peri<'static, peripherals::PA0>,
    dma_tx: Peri<'static, peripherals::GPDMA1_CH0>,
    dma_rx: Peri<'static, peripherals::GPDMA1_CH1>,
    reset: Peri<'static, peripherals::PC6>,
) -> GnssTally {
    let mut cfg = usart::Config::default();
    cfg.baudrate = 115_200;
    let Ok(mut uart) = usart::Uart::new(uart4, rx, tx, dma_tx, dma_rx, Irqs, cfg) else {
        rec(r, Group::Gnss, "gnss.uart", (Outcome::Fail, Detail::text("UART4 configuration rejected")));
        return GnssTally::default();
    };

    let mut best = GnssTally::default();
    let mut found = false;
    for baud in [115_200_u32, 460_800, 9_600, 38_400] {
        let _ = uart.set_baudrate(baud);
        let mut t = GnssTally { baud, ..GnssTally::default() };
        let mut x = Extra::default();
        listen(&mut uart, Duration::from_millis(1500), &mut t, &mut x).await;
        info!("GNSS: {} baud -> {} good, {} bad sentences", baud, t.frames_ok, t.checksum_errors);
        if t.frames_ok > best.frames_ok || best.baud == 0 {
            best = t;
        }
        if t.frames_ok >= 5 {
            found = true;
            break;
        }
    }
    rec(r, Group::Gnss, "gnss.u10.stream", gnss_stream(&best));
    rec(r, Group::Gnss, "gnss.u10.fix", gnss_fix(&best));
    if !found {
        return best;
    }
    let _ = uart.set_baudrate(best.baud);

    // Transmit path: ask for the fix interval and look for the answer.
    let mut t = GnssTally { baud: best.baud, ..GnssTally::default() };
    let mut x = Extra::default();
    send(&mut uart, Command::Pair051).await;
    listen(&mut uart, Duration::from_millis(1500), &mut t, &mut x).await;
    let res = match (x.fix_interval_ms, x.acked) {
        (Some(ms), _) => (Outcome::Pass, detail!("module answered $PAIR051: fix interval {ms} ms")),
        (None, true) => (Outcome::Pass, Detail::text("module acknowledged $PAIR051")),
        (None, false) => (
            Outcome::Fail,
            Detail::text("no answer to $PAIR051: MCU transmit path (PA0, level shifter U12, module RX) is broken"),
        ),
    };
    rec(r, Group::Gnss, "gnss.u10.tx", res);

    // Reset line: held asserted, the module must go silent; released, it must talk again.
    // Polarity assumed active high through Q2 (unverified).
    let mut rst = Output::new(reset, Level::Low, Speed::Low);
    rst.set_high();
    Timer::after_millis(100).await;
    let mut quiet = GnssTally { baud: best.baud, ..GnssTally::default() };
    let mut x = Extra::default();
    let mut tmp = [0_u8; 64];
    // drain what was in flight, then count bytes during the asserted window
    let _ = with_timeout(Duration::from_millis(50), uart.read_until_idle(&mut tmp)).await;
    listen(&mut uart, Duration::from_millis(800), &mut quiet, &mut x).await;
    rst.set_low();
    let mut again = GnssTally { baud: best.baud, ..GnssTally::default() };
    listen(&mut uart, Duration::from_millis(6000), &mut again, &mut x).await;
    let res = if quiet.frames_ok == 0 && again.frames_ok >= 3 {
        (Outcome::Pass, detail!("silent while PC6 high, {} sentences after release", again.frames_ok))
    } else if quiet.frames_ok > 0 {
        (
            Outcome::Fail,
            detail!("module kept talking ({} sentences) with PC6 high: check Q2 and R37/R38 polarity", quiet.frames_ok),
        )
    } else {
        (Outcome::Fail, detail!("module did not recover after reset ({} sentences in 6 s)", again.frames_ok))
    };
    rec(r, Group::Gnss, "gnss.u10.reset", res);
    // Leave the pin released (high impedance would be wrong if Q2 has no pull-down; keep driven low).
    core::mem::forget(rst);
    best
}

/// Count pulses on both PPS lines for `window`, with the time of each edge.
struct PpsCount {
    pulses: u32,
    first: Option<Instant>,
    last: Option<Instant>,
}

async fn count_pps(pin: &mut exti::ExtiInput<'static, Async>, window: Duration) -> PpsCount {
    let mut c = PpsCount { pulses: 0, first: None, last: None };
    let end = Instant::now() + window;
    loop {
        let now = Instant::now();
        if now >= end {
            break;
        }
        match with_timeout(end - now, pin.wait_for_rising_edge()).await {
            Ok(()) => {
                let t = Instant::now();
                c.pulses += 1;
                if c.first.is_none() {
                    c.first = Some(t);
                }
                c.last = Some(t);
            }
            Err(_) => break,
        }
    }
    c
}

async fn pps_test(
    r: &mut Rep,
    u10: &GnssTally,
    pa9: Peri<'static, peripherals::PA9>,
    exti9: Peri<'static, peripherals::EXTI9>,
    pa3: Peri<'static, peripherals::PA3>,
    exti3: Peri<'static, peripherals::EXTI3>,
    crystal: bool,
) {
    let mut p10 = exti::ExtiInput::new(pa9, exti9, Pull::Down, Irqs);
    let mut p13 = exti::ExtiInput::new(pa3, exti3, Pull::Down, Irqs);
    let window = Duration::from_millis(12_000);
    let (c10, c13) = join(count_pps(&mut p10, window), count_pps(&mut p13, window)).await;
    let ms = 12_000;
    rec(r, Group::Gnss, "gnss.u10.pps", pps(c10.pulses, ms, u10.best_quality > 0));
    rec(r, Group::Gnss, "gnss.u13.pps", pps(c13.pulses, ms, false));
    let best = if c10.pulses >= c13.pulses { &c10 } else { &c13 };
    if let (Some(a), Some(b)) = (best.first, best.last) {
        let span = (b - a).as_micros();
        rec(r, Group::System, "sys.clock", clock_vs_pps(span, best.pulses.saturating_sub(1), crystal));
    }
}

// ------------------------------------------------------------------ SD cards

/// Write a pattern into the last blocks of the card, read it back, then put the
/// original contents back.
async fn sd_write_test<D: BlockDevice>(dev: &mut D, total_blocks: u64) -> (Outcome, Detail) {
    let count = SD_TEST_BLOCKS as u64;
    let Some(first) = total_blocks.checked_sub(count).and_then(|v| u32::try_from(v).ok()) else {
        return (Outcome::Fail, Detail::text("card too small or too large for the test"));
    };
    let mut saved = [[0_u8; 512]; SD_TEST_BLOCKS];
    for (i, blk) in saved.iter_mut().enumerate() {
        if let Err(e) = dev.read_block(first.saturating_add(i as u32), blk).await {
            return (Outcome::Fail, detail!("read of block {} failed: {e:?}", i));
        }
    }
    let mut pattern = [[0_u8; 512]; SD_TEST_BLOCKS];
    for (i, blk) in pattern.iter_mut().enumerate() {
        for (k, b) in blk.iter_mut().enumerate() {
            *b = (k as u8).wrapping_mul(31).wrapping_add((i as u8).wrapping_mul(97)) ^ 0xA5;
        }
    }
    let start = Instant::now();
    let mut result = (Outcome::Pass, Detail::new());
    for (i, blk) in pattern.iter().enumerate() {
        if let Err(e) = dev.write_block(first.saturating_add(i as u32), blk).await {
            result = (Outcome::Fail, detail!("write of block {} failed: {e:?}", i));
            break;
        }
    }
    let write_us = start.elapsed().as_micros();
    if result.0 == Outcome::Pass {
        let mut check = [0_u8; 512];
        for (i, blk) in pattern.iter().enumerate() {
            if let Err(e) = dev.read_block(first.saturating_add(i as u32), &mut check).await {
                result = (Outcome::Fail, detail!("readback of block {} failed: {e:?}", i));
                break;
            }
            let (o, d) = readback(blk, &check);
            if o != Outcome::Pass {
                result = (Outcome::Fail, detail!("block {}: {}", i, d.as_str()));
                break;
            }
        }
    }
    // restore, whatever happened
    let mut restored = true;
    for (i, blk) in saved.iter().enumerate() {
        if dev.write_block(first.saturating_add(i as u32), blk).await.is_err() {
            restored = false;
        }
    }
    if result.0 == Outcome::Pass {
        let per_block = write_us.checked_div(count).unwrap_or(0);
        result.1 = detail!(
            "{} blocks written and verified, {} us per block, original contents {}",
            count,
            per_block,
            if restored { "restored" } else { "NOT RESTORED" }
        );
        if !restored {
            result.0 = Outcome::Fail;
        }
    }
    result
}

async fn sd1_test(
    r: &mut Rep,
    sdmmc1: Peri<'static, peripherals::SDMMC1>,
    clk: Peri<'static, peripherals::PC12>,
    cmd: Peri<'static, peripherals::PD2>,
    d0: Peri<'static, peripherals::PC8>,
    d1: Peri<'static, peripherals::PC9>,
    d2: Peri<'static, peripherals::PC10>,
    d3: Peri<'static, peripherals::PC11>,
    det: Peri<'static, peripherals::PC7>,
) {
    let det = Input::new(det, Pull::Up);
    rec(
        r,
        Group::Sd1,
        "sd1.detect",
        (Outcome::Info, detail!("card detect PC7 reads {} (polarity unverified)", u8::from(det.is_high()))),
    );
    let mut bus = sdmmc::Sdmmc::new_4bit(sdmmc1, Irqs, clk, cmd, d0, d1, d2, d3, Default::default());
    let mut cmd_block = sdmmc::sd::CmdBlock::new();
    let dev = match sdmmc::sd::StorageDevice::new_sd_card(&mut bus, &mut cmd_block, mhz(25)).await {
        Ok(d) => d,
        Err(e) => {
            rec(
                r,
                Group::Sd1,
                "sd1.init",
                (Outcome::Fail, detail!("card did not initialise ({e:?}): insert a card; check SDMMC1 wiring")),
            );
            return;
        }
    };
    let mut card = sd::Sd1::new(dev);
    let blocks = card.size_blocks();
    rec(r, Group::Sd1, "sd1.init", sd_card(blocks));
    let mut b0 = [0_u8; 512];
    match card.read_block(0, &mut b0).await {
        Ok(()) => rec(
            r,
            Group::Sd1,
            "sd1.read",
            (
                Outcome::Pass,
                detail!(
                    "block 0 read; boot signature {}",
                    if boot_signature(&b0) { "present (formatted card)" } else { "absent (raw or blank card)" }
                ),
            ),
        ),
        Err(e) => {
            rec(r, Group::Sd1, "sd1.read", (Outcome::Fail, detail!("read failed: {e:?}")));
            return;
        }
    }
    rec(r, Group::Sd1, "sd1.write", sd_write_test(&mut card, blocks).await);
}

async fn sd2_test(
    r: &mut Rep,
    spi2: Peri<'static, peripherals::SPI2>,
    sck: Peri<'static, peripherals::PB2>,
    mosi: Peri<'static, peripherals::PB15>,
    miso: Peri<'static, peripherals::PB14>,
    cs: Peri<'static, peripherals::PB0>,
    det: Peri<'static, peripherals::PB1>,
) {
    let det = Input::new(det, Pull::Up);
    rec(
        r,
        Group::Sd2,
        "sd2.detect",
        (Outcome::Info, detail!("card detect PB1 reads {} (polarity unverified)", u8::from(det.is_high()))),
    );
    let mut cfg = spi::Config::default();
    cfg.mode = spi::MODE_0;
    cfg.frequency = khz(400);
    let mut bus = spi::Spi::new_blocking(spi2, sck, mosi, miso, cfg);
    // 80 clocks with the card deselected, as the SD specification requires before CMD0.
    let _ = bus.blocking_write(&[0xFF_u8; 10]);
    let cs = Output::new(cs, Level::High, Speed::VeryHigh);
    let dev = match ExclusiveDevice::new_no_delay(bus, cs) {
        Ok(d) => d,
        Err(_) => return,
    };
    let mut card = sd::Sd2::new(dev, Delay);
    let blocks = match card.size_blocks() {
        Ok(b) => b,
        Err(e) => {
            rec(
                r,
                Group::Sd2,
                "sd2.init",
                (Outcome::Fail, detail!("card did not initialise ({e:?}): insert a card; check SPI2 wiring")),
            );
            return;
        }
    };
    rec(r, Group::Sd2, "sd2.init", sd_card(blocks));
    // Raise the clock now that identification is done.
    let mut fast = spi::Config::default();
    fast.mode = spi::MODE_0;
    fast.frequency = mhz(12);
    let applied = card.with_spi(|dev| dev.bus_mut().set_config(&fast).is_ok());
    if !applied {
        rec(r, Group::Sd2, "sd2.clock", (Outcome::Fail, Detail::text("could not raise the SPI clock")));
    }
    let mut b0 = [0_u8; 512];
    match card.read_block(0, &mut b0).await {
        Ok(()) => rec(
            r,
            Group::Sd2,
            "sd2.read",
            (
                Outcome::Pass,
                detail!(
                    "block 0 read; boot signature {}",
                    if boot_signature(&b0) { "present (formatted card)" } else { "absent (raw or blank card)" }
                ),
            ),
        ),
        Err(e) => {
            rec(r, Group::Sd2, "sd2.read", (Outcome::Fail, detail!("read failed: {e:?}")));
            return;
        }
    }
    rec(r, Group::Sd2, "sd2.write", sd_write_test(&mut card, blocks).await);
}

// ------------------------------------------------------------------ main

fn to_color(c: tel_core::supervisor::LedColor) -> Color {
    use tel_core::supervisor::LedColor;
    match c {
        LedColor::Off => Color::Off,
        LedColor::Red => Color::Red,
        LedColor::Green => Color::Green,
        LedColor::Amber => Color::Amber,
    }
}

fn print_summary(r: &Rep) {
    let s = r.summary();
    info!("==== SUMMARY: {} passed, {} failed, {} skipped, {} info ====", s.pass, s.fail, s.skip, s.info);
    for c in r.iter().filter(|c| c.outcome == Outcome::Fail) {
        info!("  FAILED {} - {}", c.name, c.detail.as_str());
    }
    if r.dropped() > 0 {
        info!("  report overflow: {} checks were not recorded", r.dropped());
    }
}

#[embassy_executor::main]
async fn main(_spawner: Spawner) {
    let hse_ok = probe_hse();
    let p = embassy_stm32::init(clock_config(hse_ok));
    info!("ANUFS telemetry board bring-up self-test");
    let mut report = Rep::new();
    system_checks(&mut report, hse_ok);
    button_checks(&mut report, p.PC0, p.PC1, p.PC2, p.PC3);

    // Let the IMU and LED drivers finish their power-up before talking to them.
    Timer::after_millis(60).await;

    let mut leds = led_test(&mut report, p.I2C3, p.PA8, p.PB4, p.PB5).await;
    imu_test(&mut report, p.SPI1, p.PA5, p.PA7, p.PA6, p.PA4, p.PC4, p.PC5).await;

    let (mut f1, mut rx1, mut tx1) = (p.FDCAN1, p.PB8, p.PB7);
    let (mut f2, mut rx2, mut tx2) = (p.FDCAN2, p.PB12, p.PB13);
    can_internal_test(&mut report, &mut f1, &mut rx1, &mut tx1, &mut f2, &mut rx2, &mut tx2).await;
    can_cross_test(&mut report, &mut f1, &mut rx1, &mut tx1, &mut f2, &mut rx2, &mut tx2, p.PB6, p.PB10, p.PA10).await;

    let u10 = gnss_test(&mut report, p.UART4, p.PA1, p.PA0, p.GPDMA1_CH0, p.GPDMA1_CH1, p.PC6).await;
    pps_test(&mut report, &u10, p.PA9, p.EXTI9, p.PA3, p.EXTI3, hse_ok).await;

    sd1_test(&mut report, p.SDMMC1, p.PC12, p.PD2, p.PC8, p.PC9, p.PC10, p.PC11, p.PC7).await;
    sd2_test(&mut report, p.SPI2, p.PB2, p.PB15, p.PB14, p.PB0, p.PB1).await;

    rec(
        &mut report,
        Group::System,
        "usb",
        (
            Outcome::Skip,
            Detail::text(
                "USB device is not exercised by this application; plug the cable in and check the host sees power",
            ),
        ),
    );

    // Start the watchdog: from here on the loop below must keep feeding it.
    let mut wdg = IndependentWatchdog::new(p.IWDG, 2_000_000);
    wdg.unleash();
    rec(
        &mut report,
        Group::System,
        "sys.iwdg",
        (Outcome::Info, Detail::text("watchdog started (2 s); a stuck program resets the board")),
    );

    print_summary(&report);

    // Show the result on the LEDs and repeat the summary until power-off.
    let colors = report.led_colors();
    if let Some(l) = leds.as_mut() {
        for (i, c) in colors.iter().enumerate() {
            let _ = l.leds.set(i, to_color(*c), LED_LEVEL);
        }
        let _ = l.leds.flush(&mut l.bus);
    }
    let mut tick = 0_u32;
    loop {
        wdg.pet();
        Timer::after_millis(500).await;
        tick = tick.wrapping_add(1);
        if tick.is_multiple_of(20) {
            print_summary(&report);
        }
    }
}
