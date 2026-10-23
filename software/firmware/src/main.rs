//! Telemetry logger.
//!
//! Records the GNSS fix (10 Hz), the IMU, and both CAN buses to the two SD
//! cards. All the decisions are made by the tested crates in `../crates`; this
//! file connects them to the peripherals.
//!
//! Layout:
//!
//! * High priority executor (interrupt `UART5`): GNSS, PPS capture, IMU, both
//!   CAN receivers, the SDMMC card, the supervisor, LEDs and watchdog. Hardware timers stamp the PPS edge, so
//!   task latency does not affect the clock.
//! * Thread executor (lowest priority): the blocking SPI-mode card only. A long
//!   SD2 transfer can stall nothing but itself, so the supervisor and the
//!   watchdog keep running.
//!
//! Records flow through one queue per card (`shared::CARD_RECS`); both cards
//! get every record. The supervisor (`tel_core::supervisor`) decides when
//! sessions open and when the watchdog may be fed.
//!
//! Status: compiled for the STM32H523 but never run on hardware. Run the
//! `bringup` program first.

#![no_std]
#![no_main]
#![deny(unsafe_code)]
#![allow(clippy::too_many_arguments)]

mod board;
mod sd;
mod shared;

use defmt::{info, warn};
use embassy_executor::{InterruptExecutor, Spawner};
use embassy_futures::select::{select, Either};
use embassy_stm32::exti::ExtiInput;
use embassy_stm32::gpio::{Input, Level, Output, Pull, Speed};
use embassy_stm32::interrupt::{InterruptExt, Priority};
use embassy_stm32::mode::{Async, Blocking};
use embassy_stm32::time::{khz, mhz};
use embassy_stm32::timer::input_capture::{CapturePin, InputCapture};
use embassy_stm32::timer::low_level::CountingMode;
use embassy_stm32::timer::Channel;
use embassy_stm32::usart::{RingBufferedUartRx, UartTx};
use embassy_stm32::wdg::IndependentWatchdog;
use embassy_stm32::{bind_interrupts, can, exti, i2c, interrupt, peripherals, sdmmc, spi, timer, usart, Peri};
use embassy_time::{with_timeout, Delay, Duration, Instant, Timer};
use embedded_hal_bus::spi::ExclusiveDevice;
use static_cell::StaticCell;
use tel_asm330::{Asm330, Config as ImuConfig};
use tel_can_sbg::{SbgKeys, CATALOG as SBG_CATALOG};
use tel_core::fusion::{Context, Fusion};
use tel_core::supervisor::{
    Action, Card, CardState, Config as SupConfig, Event, GnssHealth, LedColor, Supervisor, Task, LED_COUNT,
};
use tel_core::time::unix_ms;
use tel_logfmt::records::{
    event, flags, CanFrame, CanRawBatch, Event as LogEvent, Kind, SessionInfo, SignalsBatch, SCHEMA_VERSION,
};
use tel_logfmt::recovery::Geometry;
use tel_logfmt::writer::PushError;
use tel_nmea::config::{Action as CfgAction, Configurator, Target};
use tel_nmea::{parse, Command, LineAssembler, Message};
use tel_store::{recover, BlockDevice, CardLog, Service};
use tel_tlc59108::leds::{Color, ColorMap, StatusLeds};
use {defmt_rtt as _, panic_probe as _};

use shared::{card_index, publish, send_event, with, CardCmd, CARD_CMD, CARD_RECS, GNSS_START, SUP_EVENTS};

bind_interrupts!(struct Irqs {
    EXTI4 => exti::InterruptHandler<interrupt::typelevel::EXTI4>;
    TIM1_CC => timer::CaptureCompareInterruptHandler<peripherals::TIM1>;
    FDCAN1_IT0 => can::IT0InterruptHandler<peripherals::FDCAN1>;
    FDCAN1_IT1 => can::IT1InterruptHandler<peripherals::FDCAN1>;
    FDCAN2_IT0 => can::IT0InterruptHandler<peripherals::FDCAN2>;
    FDCAN2_IT1 => can::IT1InterruptHandler<peripherals::FDCAN2>;
    SDMMC1 => sdmmc::InterruptHandler<peripherals::SDMMC1>;
    UART4 => usart::InterruptHandler<peripherals::UART4>;
    GPDMA1_CHANNEL0 => embassy_stm32::dma::InterruptHandler<peripherals::GPDMA1_CH0>;
    GPDMA1_CHANNEL1 => embassy_stm32::dma::InterruptHandler<peripherals::GPDMA1_CH1>;
});

static EXECUTOR_HIGH: InterruptExecutor = InterruptExecutor::new();

// The one `unsafe` in the program: the executor's interrupt hook, as the
// embassy documentation prescribes.
#[allow(unsafe_code)]
#[interrupt]
unsafe fn UART5() {
    EXECUTOR_HIGH.on_interrupt();
}

fn now_us() -> u64 {
    Instant::now().as_micros()
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Sends a heartbeat for a task at most every 100 ms.
struct Beacon {
    task: Task,
    last_ms: u64,
}

impl Beacon {
    const fn new(task: Task) -> Self {
        Self { task, last_ms: 0 }
    }

    fn tick(&mut self) {
        let now = now_ms();
        if now.saturating_sub(self.last_ms) >= 100 || self.last_ms == 0 {
            self.last_ms = now.max(1);
            send_event(Event::Heartbeat { task: self.task, now_ms: now });
        }
    }
}

// ------------------------------------------------------------------ PPS

/// Time-stamps the pulse per second of the GNSS module on the supervisor clock.
/// The timer latches the counter at the edge; the age of that capture is
/// subtracted from the monotonic clock read a moment later, so the answer does
/// not depend on interrupt latency (as long as it is under 65 ms).
#[embassy_executor::task]
async fn pps_task(tim1: Peri<'static, peripherals::TIM1>, pa9: Peri<'static, peripherals::PA9>) {
    let pin = CapturePin::new(pa9, Pull::Down);
    let mut ic = InputCapture::new(tim1, None, Some(pin), None, None, Irqs, mhz(1), CountingMode::EdgeAlignedUp);
    loop {
        let captured = ic.wait_for_rising_edge(Channel::Ch2).await;
        let counter = embassy_stm32::pac::TIM1.cnt().read();
        let now = now_us();
        let age = u64::from((counter.0 as u16).wrapping_sub(captured));
        let at = now.saturating_sub(age);
        with(|s| {
            s.clock.on_pps(at);
        });
    }
}

// ------------------------------------------------------------------ GNSS

/// Receives NMEA, keeps the clock and the fusion state, and publishes the
/// periodic sample.
struct Stream {
    asm: LineAssembler,
    fusion: Fusion,
    keys: SbgKeys,
    dirty: bool,
    last_sample_us: u64,
    health: GnssHealth,
    frames_ok: u32,
}

impl Stream {
    fn new() -> Option<Self> {
        let keys = SbgKeys::resolve(&SBG_CATALOG).ok()?;
        Some(Self {
            asm: LineAssembler::new(),
            fusion: Fusion::new(board::FIX_INTERVAL_MS),
            keys,
            dirty: false,
            last_sample_us: 0,
            health: GnssHealth::default(),
            frames_ok: 0,
        })
    }

    /// Run bytes through the framer and decoder.
    fn feed(&mut self, bytes: &[u8], at_us: u64, mut cfg: Option<&mut Configurator>) {
        for &byte in bytes {
            let parsed = match self.asm.push(byte) {
                Some(Ok(sentence)) => Some(parse(&sentence)),
                Some(Err(_)) | None => None,
            };
            let Some(Ok(msg)) = parsed else { continue };
            self.frames_ok = self.frames_ok.saturating_add(1);
            if let Some(c) = cfg.as_deref_mut() {
                c.on_message(&msg);
            }
            self.handle(msg, at_us);
        }
    }

    fn handle(&mut self, msg: Message, at_us: u64) {
        match msg {
            Message::Rmc(r) => {
                // The time of this sentence names the pulse that just happened.
                if let (Some(date), Some(time)) = (r.date, r.time) {
                    if let Some(ms) = unix_ms(date, time) {
                        with(|s| s.clock.on_time_message(ms, at_us));
                    }
                }
                self.fusion.on_rmc(r, at_us);
                self.dirty = true;
            }
            Message::Gga(g) => {
                self.fusion.on_gga(g, at_us);
                self.dirty = true;
            }
            Message::Tar(t) => {
                self.fusion.on_tar(t, at_us);
                self.dirty = true;
            }
            _ => {}
        }
    }

    /// Keep the log carrying IMU and CAN data at 5 Hz even when no GNSS data is arriving.
    fn publish_if_due(&mut self) {
        let t = now_us();
        if t.saturating_sub(self.last_sample_us) >= 200_000 {
            self.publish_sample(t);
        }
    }

    /// Build one sample and queue it, together with the newest SBG signals.
    fn publish_sample(&mut self, at_us: u64) {
        self.dirty = false;
        self.last_sample_us = at_us;
        let nmea_errors = self.asm.stats().checksum_errors;
        let ctx = with(|s| {
            let mut f = s.ext_flags;
            if s.can_alive.first().copied().unwrap_or(false) {
                f |= flags::CAN1_ALIVE;
            }
            if s.can_alive.get(1).copied().unwrap_or(false) {
                f |= flags::CAN2_ALIVE;
            }
            if s.clock.is_locked(at_us) {
                f |= flags::PPS_LOCKED;
            }
            Context {
                now_us: at_us,
                utc_unix_ms: s.clock.utc_unix_ms_at(at_us),
                imu: s.imu.take(),
                flags: f,
                nmea_errors,
                overruns: s.overruns.iter().fold(0_u32, |a, b| a.saturating_add(*b)),
                can1_frames: s.can_frames.first().copied().unwrap_or(0),
                can2_frames: s.can_frames.get(1).copied().unwrap_or(0),
            }
        });
        let sample = self.fusion.build(&ctx);
        let mut body = [0_u8; 255];
        if let Ok(n) = sample.encode(&mut body) {
            publish(Kind::Sample, body.get(..n).unwrap_or(&[]));
        }

        let mut batch = SignalsBatch::new(2, at_us);
        with(|s| {
            for id in self.keys.all() {
                if let Some(v) = s.sbg.get(id) {
                    batch.try_push(id.0, v.raw);
                }
            }
        });
        if !batch.is_empty() {
            let mut sig = [0_u8; 255];
            if let Ok(n) = batch.encode(&mut sig) {
                publish(Kind::Signals, sig.get(..n).unwrap_or(&[]));
            }
        }

        let health = GnssHealth {
            configured: self.health.configured,
            fix: sample.flags & flags::GNSS_VALID != 0,
            rtk_fixed: sample.fix_quality == 4,
            heading: sample.flags & flags::HEADING_VALID != 0,
        };
        self.set_health(health);
    }

    fn set_health(&mut self, health: GnssHealth) {
        if health != self.health {
            self.health = health;
            send_event(Event::Gnss(health));
        }
    }
}

const CMD_TIMEOUT: Duration = Duration::from_millis(2_000);

/// Longest a loop waits before it reports that it is alive.
const BEAT_SLICE: Duration = Duration::from_millis(100);

async fn send_command(tx: &mut UartTx<'static, Async>, cmd: Command<'_>) {
    let mut out = [0_u8; 96];
    if let Ok(n) = cmd.encode(&mut out) {
        let _ = tx.write(out.get(..n).unwrap_or(&[])).await;
    }
}

/// Drive a [`Configurator`] to completion.
async fn run_configurator(
    c: &mut Configurator,
    ring: &mut RingBufferedUartRx<'static>,
    tx: &mut UartTx<'static, Async>,
    st: &mut Stream,
    beacon: &mut Beacon,
) -> Result<tel_nmea::config::Outcome, tel_nmea::config::ConfigError> {
    let mut deadline = Instant::now();
    let mut buf = [0_u8; 128];
    loop {
        beacon.tick();
        match c.poll() {
            CfgAction::Send(cmd) => {
                send_command(tx, cmd).await;
                deadline = Instant::now() + CMD_TIMEOUT;
            }
            CfgAction::Done(o) => return Ok(o),
            CfgAction::Failed(e) => return Err(e),
            CfgAction::Wait => {
                let now = Instant::now();
                if now >= deadline {
                    c.on_timeout();
                    continue;
                }
                match with_timeout((deadline - now).min(BEAT_SLICE), ring.read(&mut buf)).await {
                    Ok(Ok(n)) => st.feed(buf.get(..n).unwrap_or(&[]), now_us(), Some(&mut *c)),
                    Ok(Err(_)) => {}
                    // Only a timeout of the whole command counts, not one slice of the wait.
                    Err(_) if Instant::now() >= deadline => c.on_timeout(),
                    Err(_) => {}
                }
                st.publish_if_due();
            }
        }
    }
}

/// Look for the baud rate the module is talking at.
async fn find_baud(
    ring: &mut RingBufferedUartRx<'static>,
    tx: &mut UartTx<'static, Async>,
    st: &mut Stream,
    beacon: &mut Beacon,
) -> Option<u32> {
    let mut buf = [0_u8; 128];
    for baud in [board::GNSS_BAUD, 115_200, 9_600, 38_400] {
        let _ = ring.set_baudrate(baud);
        let _ = tx.set_baudrate(baud);
        let before = st.frames_ok;
        let end = Instant::now() + Duration::from_millis(1_200);
        while Instant::now() < end {
            beacon.tick();
            let left = (end - Instant::now()).min(BEAT_SLICE);
            if let Ok(Ok(n)) = with_timeout(left, ring.read(&mut buf)).await {
                st.feed(buf.get(..n).unwrap_or(&[]), now_us(), None);
            }
            st.publish_if_due();
        }
        if st.frames_ok.saturating_sub(before) >= 3 {
            return Some(baud);
        }
    }
    None
}

/// Verify (and if needed apply) the module settings. True when the module ends
/// up configured as required.
async fn configure_module(
    baud: u32,
    ring: &mut RingBufferedUartRx<'static>,
    tx: &mut UartTx<'static, Async>,
    reset: &mut Output<'static>,
    st: &mut Stream,
    beacon: &mut Beacon,
) -> bool {
    use tel_nmea::config::Outcome;
    let target = Target { fix_interval_ms: board::FIX_INTERVAL_MS, baud: board::GNSS_BAUD, tar_rate: board::TAR_RATE };
    let Ok(mut c) = Configurator::new(target, baud) else { return false };
    match run_configurator(&mut c, ring, tx, st, beacon).await {
        Ok(Outcome::AlreadyConfigured) => true,
        Ok(Outcome::Applied { new_baud }) => {
            info!("GNSS: settings written, restarting the module at {} baud", new_baud);
            // Restart (assumed active high through Q2; unverified), then verify.
            reset.set_high();
            Timer::after_millis(100).await;
            reset.set_low();
            let _ = ring.set_baudrate(new_baud);
            let _ = tx.set_baudrate(new_baud);
            let end = Instant::now() + Duration::from_millis(3_000);
            let mut buf = [0_u8; 128];
            while Instant::now() < end {
                beacon.tick();
                let left = (end - Instant::now()).min(BEAT_SLICE);
                if let Ok(Ok(n)) = with_timeout(left, ring.read(&mut buf)).await {
                    st.feed(buf.get(..n).unwrap_or(&[]), now_us(), None);
                }
                st.publish_if_due();
            }
            let Ok(mut verify) = Configurator::new(target, new_baud) else { return false };
            matches!(run_configurator(&mut verify, ring, tx, st, beacon).await, Ok(Outcome::AlreadyConfigured))
        }
        Err(e) => {
            warn!("GNSS: configuration failed: {}", defmt::Debug2Format(&e));
            false
        }
    }
}

#[embassy_executor::task]
async fn gnss_task(
    uart4: Peri<'static, peripherals::UART4>,
    rx: Peri<'static, peripherals::PA1>,
    tx: Peri<'static, peripherals::PA0>,
    dma_tx: Peri<'static, peripherals::GPDMA1_CH0>,
    dma_rx: Peri<'static, peripherals::GPDMA1_CH1>,
    reset: Peri<'static, peripherals::PC6>,
) {
    let mut cfg = usart::Config::default();
    cfg.baudrate = board::GNSS_BAUD;
    let uart = match usart::Uart::new(uart4, rx, tx, dma_tx, dma_rx, Irqs, cfg) {
        Ok(u) => u,
        Err(_) => {
            warn!("GNSS: UART4 configuration rejected");
            send_event(Event::GnssFailed);
            return;
        }
    };
    let (mut utx, urx) = uart.split();
    static RING: StaticCell<[u8; 4096]> = StaticCell::new();
    let mut ring = urx.into_ring_buffered(RING.init([0; 4096]));
    ring.start_uart();
    let mut reset = Output::new(reset, Level::Low, Speed::Low);
    let mut beacon = Beacon::new(Task::Gnss);
    let Some(mut st) = Stream::new() else {
        warn!("GNSS: SBG signal table does not resolve");
        send_event(Event::GnssFailed);
        return;
    };

    // Find out how the module is talking, then stream until asked to configure it.
    let mut baud = find_baud(&mut ring, &mut utx, &mut st, &mut beacon).await;
    match baud {
        Some(b) => info!("GNSS: module found at {} baud", b),
        None => warn!("GNSS: no module output heard yet"),
    }
    let mut started = false;
    let mut buf = [0_u8; 128];
    loop {
        beacon.tick();
        if !started && GNSS_START.signaled() {
            started = true;
            GNSS_START.reset();
            let ok = match baud {
                Some(b) => configure_module(b, &mut ring, &mut utx, &mut reset, &mut st, &mut beacon).await,
                None => false,
            };
            if ok {
                baud = Some(board::GNSS_BAUD);
                let mut h = st.health;
                h.configured = true;
                st.set_health(h);
                info!("GNSS: configured for {} ms fixes", board::FIX_INTERVAL_MS);
            } else {
                send_event(Event::GnssFailed);
            }
        }
        if baud.is_none() {
            // Keep looking; the module may simply have been slow to power up.
            baud = find_baud(&mut ring, &mut utx, &mut st, &mut beacon).await;
        }
        match with_timeout(Duration::from_millis(15), ring.read(&mut buf)).await {
            Ok(Ok(n)) => st.feed(buf.get(..n).unwrap_or(&[]), now_us(), None),
            Ok(Err(_)) => {}
            Err(_) => {
                // Quiet for 15 ms: the burst of sentences for this epoch is complete.
                if st.dirty {
                    st.publish_sample(now_us());
                }
            }
        }
        st.publish_if_due();
    }
}

// ------------------------------------------------------------------ IMU

#[embassy_executor::task]
async fn imu_task(
    spi1: Peri<'static, peripherals::SPI1>,
    sck: Peri<'static, peripherals::PA5>,
    mosi: Peri<'static, peripherals::PA7>,
    miso: Peri<'static, peripherals::PA6>,
    cs: Peri<'static, peripherals::PA4>,
    int1: Peri<'static, peripherals::PC4>,
    exti4: Peri<'static, peripherals::EXTI4>,
) {
    let mut cfg = spi::Config::default();
    cfg.mode = spi::MODE_3;
    cfg.frequency = mhz(8);
    let bus = spi::Spi::new_blocking(spi1, sck, mosi, miso, cfg);
    let cs = Output::new(cs, Level::High, Speed::VeryHigh);
    let Ok(dev) = ExclusiveDevice::new_no_delay(bus, cs);
    let mut imu = Asm330::new(dev);
    let mut pin = ExtiInput::new(int1, exti4, Pull::None, Irqs);
    let mut beacon = Beacon::new(Task::Imu);
    let mut ok = false;
    let mut failures = 0_u32;
    Timer::after_millis(u64::from(tel_asm330::TURN_ON_MS) + 5).await;
    loop {
        beacon.tick();
        if !ok {
            match imu.init(ImuConfig::default(), &mut Delay) {
                Ok(()) => {
                    ok = true;
                    failures = 0;
                    send_event(Event::ImuOk { ok: true });
                    info!("IMU: configured");
                }
                Err(_) => {
                    send_event(Event::ImuOk { ok: false });
                    Timer::after_millis(200).await;
                }
            }
            continue;
        }
        // Data-ready wakes us; a timeout means the line is dead, so poll instead.
        let _ = with_timeout(Duration::from_millis(50), pin.wait_for_rising_edge()).await;
        match imu.status() {
            Ok(s) if s.accel_ready || s.gyro_ready => match imu.read_scaled() {
                Ok(v) => {
                    failures = 0;
                    with(|sh| sh.imu.add(&v));
                }
                Err(_) => failures = failures.saturating_add(1),
            },
            Ok(_) => {}
            Err(_) => failures = failures.saturating_add(1),
        }
        if failures >= 20 {
            ok = false;
            send_event(Event::ImuOk { ok: false });
            warn!("IMU: lost, reinitialising");
        }
    }
}

// ------------------------------------------------------------------ CAN

/// Receives one bus, never transmits. The controller runs in restricted
/// operation mode: it acknowledges valid frames (an Ellipse needs a listener
/// to acknowledge it) but cannot send anything.
#[embassy_executor::task(pool_size = 2)]
async fn can_task(bus: u8, mut can: can::Can<'static>) {
    let index = usize::from(bus.saturating_sub(1)).min(1);
    let mut batch = CanRawBatch::new(bus);
    let mut batch_started = Instant::now();
    let mut last_rx: Option<Instant> = None;
    let mut alive = false;
    loop {
        if let Ok(Ok(env)) = with_timeout(Duration::from_millis(25), can.read()).await {
            {
                let t = now_us();
                last_rx = Some(Instant::now());
                let (id, extended) = match env.frame.id() {
                    embedded_can::Id::Standard(s) => (u32::from(s.as_raw()), false),
                    embedded_can::Id::Extended(e) => (e.as_raw(), true),
                };
                let data = env.frame.data();
                with(|s| {
                    if let Some(c) = s.can_frames.get_mut(index) {
                        *c = c.saturating_add(1);
                    }
                    if bus == 2 {
                        tel_can::ingest(&SBG_CATALOG, &mut s.sbg, id, extended, data, t);
                    }
                });
                let mut bytes = [0_u8; 8];
                let n = data.len().min(8);
                if let (Some(dst), Some(src)) = (bytes.get_mut(..n), data.get(..n)) {
                    dst.copy_from_slice(src);
                }
                let frame = CanFrame { t_us: t, id, extended, dlc: u8::try_from(n).unwrap_or(8), data: bytes };
                if batch.is_empty() {
                    batch_started = Instant::now();
                }
                if !batch.try_push(&frame) {
                    flush_batch(&mut batch);
                    batch_started = Instant::now();
                    let _ = batch.try_push(&frame);
                }
            }
        }
        if !batch.is_empty() && batch_started.elapsed() >= Duration::from_millis(100) {
            flush_batch(&mut batch);
        }
        let now_alive = last_rx.is_some_and(|t| t.elapsed() < Duration::from_millis(500));
        if now_alive != alive {
            alive = now_alive;
            with(|s| {
                if let Some(a) = s.can_alive.get_mut(index) {
                    *a = alive;
                }
            });
            send_event(Event::CanAlive { bus, alive });
        }
    }
}

fn flush_batch(batch: &mut CanRawBatch) {
    let mut body = [0_u8; 255];
    if let Ok(n) = batch.encode(&mut body) {
        publish(Kind::CanRaw, body.get(..n).unwrap_or(&[]));
    }
    batch.clear();
}

// ------------------------------------------------------------------ cards

/// Wait for a command while still reporting that the logger is alive.
async fn wait_cmd(card: Card, beacon: &mut Beacon) -> CardCmd {
    let signal = &CARD_CMD[card_index(card)];
    loop {
        beacon.tick();
        if let Ok(cmd) = with_timeout(Duration::from_millis(200), signal.wait()).await {
            return cmd;
        }
    }
}

/// Wait for the supervisor to ask for a new mount attempt.
async fn wait_mount(card: Card, beacon: &mut Beacon) {
    while wait_cmd(card, beacon).await != CardCmd::Mount {}
}

fn push_event(log: &mut CardLog<8>, code: u16, arg: u32) {
    let ev = LogEvent { t_us: now_us(), code, arg };
    let mut body = [0_u8; 32];
    if let Ok(n) = ev.encode(&mut body) {
        let _ = log.push(Kind::Event, body.get(..n).unwrap_or(&[]));
    }
}

/// Write the records of one session. Returns when the card can no longer be
/// written (fault, or the region is full).
async fn run_session<D: BlockDevice>(card: Card, dev: &mut D, geo: Geometry, beacon: &mut Beacon) {
    session(card, dev, geo, beacon).await;
    let index = card_index(card);
    with(|s| {
        if let Some(r) = s.recording.get_mut(index) {
            *r = false;
        }
    });
}

async fn session<D: BlockDevice>(card: Card, dev: &mut D, geo: Geometry, beacon: &mut Beacon) {
    let index = card_index(card);
    let rec = match recover(dev, geo).await {
        Ok(r) => r,
        Err(_) => return,
    };
    let Ok(mut log) = CardLog::<8>::open(geo, &rec) else { return };
    // Anything queued before the session opened belongs to no session.
    while CARD_RECS[index].try_receive().is_ok() {}
    with(|s| {
        if let Some(r) = s.recording.get_mut(index) {
            *r = true;
        }
    });

    let utc = with(|s| s.clock.utc_unix_ms_at(now_us())).unwrap_or(0);
    let info = SessionInfo {
        schema: SCHEMA_VERSION,
        firmware: board::FIRMWARE,
        fix_interval_ms: board::FIX_INTERVAL_MS,
        can1_bitrate: board::CAN1_BITRATE,
        can2_bitrate: board::CAN2_BITRATE,
        imu_odr_hz: board::IMU_ODR_HZ,
        t_us: now_us(),
        utc_unix_ms: utc,
    };
    let mut body = [0_u8; 64];
    if let Ok(n) = info.encode(&mut body) {
        let _ = log.push(Kind::SessionInfo, body.get(..n).unwrap_or(&[]));
    }
    push_event(&mut log, event::BOOT, 0);
    info!("card {}: session {} open", card.number(), log.session());

    let mut last_flush = Instant::now();
    loop {
        beacon.tick();
        let first = with_timeout(Duration::from_millis(200), CARD_RECS[index].receive()).await.ok();
        let mut next = first;
        while let Some(r) = next {
            if !push_record(&mut log, dev, &r).await {
                return;
            }
            next = CARD_RECS[index].try_receive().ok();
        }
        // Bound the data at risk in a power cut: close the open block once a second.
        if last_flush.elapsed() >= Duration::from_secs(1) {
            let _ = log.flush();
            last_flush = Instant::now();
        }
        if matches!(log.service(dev).await, Service::Faulted) {
            return;
        }
    }
}

/// Append a record, writing out blocks if the ring is full. False when the
/// session can no longer continue.
async fn push_record<D: BlockDevice>(log: &mut CardLog<8>, dev: &mut D, rec: &shared::Rec) -> bool {
    for _ in 0..2 {
        match log.push(rec.kind, rec.bytes()) {
            Ok(()) => return true,
            Err(PushError::Overrun) => {
                if matches!(log.service(dev).await, Service::Faulted) {
                    return false;
                }
            }
            // Region full, faulted or closed: the session is over.
            Err(PushError::Full | PushError::Faulted | PushError::Closed) => return false,
            // A record too large for the format cannot be stored; skip it.
            Err(PushError::TooLarge) => return true,
        }
    }
    // Still full after writing: the card is too slow. Drop this record.
    true
}

/// What happens on one card from mount to fault. Returns when the card must be
/// mounted again.
async fn card_cycle<D: BlockDevice>(card: Card, dev: &mut D, total_blocks: u64, beacon: &mut Beacon) {
    let geo = Geometry { start_lba: 0, blocks: u32::try_from(total_blocks).unwrap_or(u32::MAX) };
    #[allow(unused_mut)] // only mutated when the auto-prepare feature is on
    let mut mounted = recover(dev, geo).await;
    #[cfg(feature = "auto-prepare")]
    if mounted.is_err() {
        // The card id comes from the device id, so each board numbers its cards the same way.
        let uid = embassy_stm32::uid::uid();
        let id = uid.iter().fold(0x811C_9DC5_u32, |h, b| (h ^ u32::from(*b)).wrapping_mul(0x0100_0193));
        warn!("card {}: formatting a new log region (auto-prepare)", card.number());
        if tel_store::prepare(dev, geo, id ^ card.number()).await.is_ok() {
            mounted = recover(dev, geo).await;
        }
    }
    match mounted {
        Ok(r) => {
            info!("card {}: log region found, last session {}", card.number(), r.last_session);
            send_event(Event::CardMounted { card, ok: true });
        }
        Err(_) => {
            warn!("card {}: no prepared log region (run `tel-decode prepare`)", card.number());
            send_event(Event::CardMounted { card, ok: false });
            wait_mount(card, beacon).await;
            return;
        }
    }
    match wait_cmd(card, beacon).await {
        CardCmd::Mount => {}
        CardCmd::Open => {
            run_session(card, dev, geo, beacon).await;
            warn!("card {}: session ended", card.number());
            send_event(Event::WriteFailed { card });
            wait_mount(card, beacon).await;
        }
    }
}

#[embassy_executor::task]
async fn sd1_task(
    sdmmc1: Peri<'static, peripherals::SDMMC1>,
    clk: Peri<'static, peripherals::PC12>,
    cmd: Peri<'static, peripherals::PD2>,
    d0: Peri<'static, peripherals::PC8>,
    d1: Peri<'static, peripherals::PC9>,
    d2: Peri<'static, peripherals::PC10>,
    d3: Peri<'static, peripherals::PC11>,
) {
    let mut bus = sdmmc::Sdmmc::new_4bit(sdmmc1, Irqs, clk, cmd, d0, d1, d2, d3, Default::default());
    let mut cmd_block = sdmmc::sd::CmdBlock::new();
    let mut beacon = Beacon::new(Task::Logger);
    loop {
        match sdmmc::sd::StorageDevice::new_sd_card(&mut bus, &mut cmd_block, mhz(25)).await {
            Ok(dev) => {
                let mut card = sd::Sd1::new(dev);
                let blocks = card.size_blocks();
                card_cycle(Card::Sd1, &mut card, blocks, &mut beacon).await;
            }
            Err(_) => {
                warn!("card 1: did not initialise");
                send_event(Event::CardMounted { card: Card::Sd1, ok: false });
                wait_mount(Card::Sd1, &mut beacon).await;
            }
        }
    }
}

#[embassy_executor::task]
async fn sd2_task(
    spi2: Peri<'static, peripherals::SPI2>,
    sck: Peri<'static, peripherals::PB2>,
    mosi: Peri<'static, peripherals::PB15>,
    miso: Peri<'static, peripherals::PB14>,
    cs: Peri<'static, peripherals::PB0>,
) {
    let mut cfg = spi::Config::default();
    cfg.mode = spi::MODE_0;
    cfg.frequency = khz(400);
    let mut bus = spi::Spi::new_blocking(spi2, sck, mosi, miso, cfg);
    // The card needs 74 clocks at a low rate with chip select high before the first command.
    let _ = bus.blocking_write(&[0xFF_u8; 10]);
    let cs = Output::new(cs, Level::High, Speed::VeryHigh);
    let Ok(dev) = ExclusiveDevice::new_no_delay(bus, cs);
    let mut card = sd::Sd2::new(dev, Delay);
    let mut beacon = Beacon::new(Task::Logger);
    loop {
        card.mark_uninit();
        match card.size_blocks() {
            Ok(blocks) => {
                // Identification is done: raise the clock for data.
                let mut fast = spi::Config::default();
                fast.mode = spi::MODE_0;
                fast.frequency = mhz(12);
                card.with_spi(|d| {
                    let _ = d.bus_mut().set_config(&fast);
                });
                card_cycle(Card::Sd2, &mut card, blocks, &mut beacon).await;
            }
            Err(_) => {
                warn!("card 2: did not initialise");
                send_event(Event::CardMounted { card: Card::Sd2, ok: false });
                wait_mount(Card::Sd2, &mut beacon).await;
            }
        }
    }
}

// ------------------------------------------------------------------ supervisor, LEDs, watchdog

struct Leds {
    bus: i2c::I2c<'static, Blocking, i2c::Master>,
    leds: StatusLeds,
    _reset: Output<'static>,
}

fn to_color(c: LedColor) -> Color {
    match c {
        LedColor::Off => Color::Off,
        LedColor::Red => Color::Red,
        LedColor::Green => Color::Green,
        LedColor::Amber => Color::Amber,
    }
}

fn apply_action(a: Action) {
    match a {
        Action::StartGnssSetup => GNSS_START.signal(()),
        Action::RetryMount(card) => CARD_CMD[card_index(card)].signal(CardCmd::Mount),
        Action::OpenSession(card) => CARD_CMD[card_index(card)].signal(CardCmd::Open),
        Action::LogEvent { code, arg } => {
            let ev = LogEvent { t_us: now_us(), code, arg };
            let mut body = [0_u8; 32];
            if let Ok(n) = ev.encode(&mut body) {
                publish(Kind::Event, body.get(..n).unwrap_or(&[]));
            }
        }
    }
}

#[embassy_executor::task]
async fn supervisor_task(mut leds: Option<Leds>, iwdg: Peri<'static, peripherals::IWDG>) {
    let mut sup = Supervisor::new(SupConfig::default());
    let mut beacon = Beacon::new(Task::Supervisor);
    let mut shown = [LedColor::Off; LED_COUNT];
    let mut first_paint = true;
    let mut last_overruns = 0_u32;
    let mut wdg = IndependentWatchdog::new(iwdg, 3_000_000);
    wdg.unleash();
    for a in sup.step(Event::Tick { now_ms: 0 }).iter() {
        apply_action(a);
    }
    loop {
        // Wake for an event, or after 100 ms for the periodic tick.
        let first = match select(SUP_EVENTS.receive(), Timer::after_millis(100)).await {
            Either::First(ev) => Some(ev),
            Either::Second(()) => None,
        };
        let mut pending = first;
        while let Some(ev) = pending {
            for a in sup.step(ev).iter() {
                apply_action(a);
            }
            pending = SUP_EVENTS.try_receive().ok();
        }
        beacon.tick();
        let now = now_ms();
        for a in sup.step(Event::Tick { now_ms: now }).iter() {
            apply_action(a);
        }

        // Flags every sample should carry.
        let mut f = 0_u32;
        if sup.no_fix_start() {
            f |= flags::NO_FIX_START;
        }
        if sup.card_state(Card::Sd1) == CardState::Faulted || sup.card_state(Card::Sd2) == CardState::Faulted {
            f |= flags::CARD_FAULT;
        }
        let overruns = sup.overruns();
        if overruns != last_overruns {
            f |= flags::OVERRUN;
            last_overruns = overruns;
        }
        with(|s| s.ext_flags = f);

        if sup.should_kick_watchdog(now) {
            wdg.pet();
        }

        let colors = sup.leds(now);
        if let Some(l) = leds.as_mut() {
            if first_paint || colors != shown {
                for (i, c) in colors.iter().enumerate() {
                    let _ = l.leds.set(i, to_color(*c), board::LED_LEVEL);
                }
                if l.leds.flush(&mut l.bus).is_ok() {
                    shown = colors;
                    first_paint = false;
                } else {
                    // Bring the drivers back if they lost their settings.
                    let _ = l.leds.init(&mut l.bus, &mut Delay);
                    first_paint = true;
                }
            }
        }
    }
}

// ------------------------------------------------------------------ main

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let hse_ok = board::probe_hse();
    let p = embassy_stm32::init(board::clock_config(hse_ok));
    info!("telemetry logger starting (crystal {})", if hse_ok { "ok" } else { "FAILED, using internal oscillator" });

    // Status LEDs. If the drivers are missing the logger still runs.
    let reset = Output::new(p.PB5, Level::Low, Speed::Low);
    Timer::after_millis(2).await;
    let mut reset = reset;
    reset.set_high();
    Timer::after_millis(5).await;
    let mut i2c_cfg = i2c::Config::default();
    i2c_cfg.frequency = khz(100);
    let mut bus = i2c::I2c::new_blocking(p.I2C3, p.PA8, p.PB4, i2c_cfg);
    let mut status = StatusLeds::new(ColorMap { red: board::RED_BANK });
    let leds = match status.init(&mut bus, &mut Delay) {
        Ok(()) => Some(Leds { bus, leds: status, _reset: reset }),
        Err(_) => {
            warn!("LED drivers did not answer");
            core::mem::forget(reset);
            None
        }
    };

    // CAN: receive only. SIL (PA10) low keeps the CAN2 transceiver in normal mode
    // so it can acknowledge frames (polarity unverified).
    let sil = Output::new(p.PA10, Level::Low, Speed::Low);
    core::mem::forget(sil);
    let mut c1 = can::CanConfigurator::new(p.FDCAN1, p.PB8, p.PB7, Irqs);
    c1.set_bitrate(board::CAN1_BITRATE);
    let can1 = c1.start(can::OperatingMode::RestrictedOperationMode);
    let mut c2 = can::CanConfigurator::new(p.FDCAN2, p.PB12, p.PB13, Irqs);
    c2.set_bitrate(board::CAN2_BITRATE);
    let can2 = c2.start(can::OperatingMode::RestrictedOperationMode);

    // The card detect pins are not used: a mount attempt shows whether a card is there.
    let _ = Input::new(p.PC7, Pull::Up);
    let _ = Input::new(p.PB1, Pull::Up);

    // High priority executor.
    interrupt::UART5.set_priority(Priority::P6);
    let high = EXECUTOR_HIGH.start(interrupt::UART5);
    high.spawn(pps_task(p.TIM1, p.PA9).unwrap());
    high.spawn(gnss_task(p.UART4, p.PA1, p.PA0, p.GPDMA1_CH0, p.GPDMA1_CH1, p.PC6).unwrap());
    high.spawn(imu_task(p.SPI1, p.PA5, p.PA7, p.PA6, p.PA4, p.PC4, p.EXTI4).unwrap());
    high.spawn(can_task(1, can1).unwrap());
    high.spawn(can_task(2, can2).unwrap());
    high.spawn(sd1_task(p.SDMMC1, p.PC12, p.PD2, p.PC8, p.PC9, p.PC10, p.PC11).unwrap());

    high.spawn(supervisor_task(leds, p.IWDG).unwrap());

    // Thread mode: the blocking card, which may hold the CPU for tens of milliseconds.
    spawner.spawn(sd2_task(p.SPI2, p.PB2, p.PB15, p.PB14, p.PB0).unwrap());
}
