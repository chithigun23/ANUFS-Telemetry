# Firmware crate

The MCU-side crate. It is built separately from the PC workspace (it is listed under `exclude` in `../Cargo.toml`) because it targets the chip.

```
rustup target add thumbv8m.main-none-eabihf
cargo build --release
```

Two programs are built from it:

| Program | Purpose | Status |
|---|---|---|
| `bringup` (`src/bin/bringup.rs`) | Checks every feature and device on a freshly assembled board | Compiles; never run on hardware |
| `telemetry-firmware` (`src/main.rs`) | The logger the board exists for | Compiles; never run on hardware |

## Running the bring-up test

1. Assemble the board in the stages of `../../hardware/ASSEMBLY.md`.
2. Fit a card in each SD slot (the test saves and restores the blocks it uses, but use a card you can afford to lose), connect the GNSS antennas (ANN-MB5 on J7 and J8, with sky view if you want a fix), and put the board flat and still on the bench.
3. Connect the debug probe to J12 and power the board from USB-C or J1 (J12 has no 3.3 V pin).
4. Run:

```
cd software/firmware
cargo run --release --bin bringup
```

`probe-rs` flashes the board and prints the log (the chip name `STM32H523RC` is the one `probe-rs chip list` reports). Each check prints one line:

```
[PASS] imu.whoami - WHO_AM_I = 0x6b
[FAIL] gnss.u10.stream - no data at 460800 baud (check UART4, level shifter U12, module power)
```

Colours on the five status LEDs at the end: green = everything in that group passed, red = at least one check failed, dark = nothing ran. In order: 0 system (clock, buttons, watchdog), 1 GNSS, 2 SD1, 3 SD2, 4 sensors (IMU, LED drivers, CAN).

### What it checks

| Group | Check | Pass when |
|---|---|---|
| System | `sys.hse` | The 24 MHz crystal starts (tested before the HAL commits to it, so a bad crystal gives a report, not a hang) |
| System | `sys.clock` | The MCU clock agrees with the GNSS pulse-per-second to within 200 ppm (needs a fix) |
| System | `sys.switches` | Information only: levels of the SW1 and SW2 pins |
| Sensors | `led.i2c`, `led.verify` | Both TLC59108 answer at 0x40 and 0x41 and read back what was written |
| Sensors | colour test | Information: lights each driver bank in turn, then both, then each LED alone; **look at the board** to see which bank is red |
| Sensors | `imu.whoami`, `imu.init`, `imu.int1`, `imu.static` | WHO_AM_I is 0x6B, the registers read back, INT1 signals data ready, and at rest the acceleration magnitude is 0.85 to 1.15 g with small gyro offset and a plausible temperature |
| Sensors | `can1.loopback`, `can2.loopback` | 32 frames each come back intact through the controller's internal loopback |
| Sensors | `can.cross` | With J4 joined to J5 (CAN_H to CAN_H, CAN_L to CAN_L, 120 ohm across the pair), frames cross both ways through both transceivers. Skipped if nothing crosses |
| GNSS | `gnss.u10.stream` | NMEA arrives with under 5 % bad checksums (tries 115200, 460800, 9600 and 38400 baud) |
| GNSS | `gnss.u10.tx` | The module answers `$PAIR051`, which proves the MCU-to-module path |
| GNSS | `gnss.u10.reset` | The module goes silent while PC6 is held and talks again after release |
| GNSS | `gnss.u10.pps`, `gnss.u13.pps` | About one pulse per second over 12 s (information only without a fix) |
| SD1, SD2 | init, read, write | The card reports a plausible size, block 0 reads, and 16 blocks at the end of the card can be written, read back and then restored |
| System | `usb` | Skipped: not exercised by this program |

The thresholds are in `../crates/tel-selftest`, where they are unit tested on the PC (27 tests).

### Things the test cannot know yet

These come from the schematic, not from a working board, so a failure here may be the test and not the board:

- **Which LED driver pin is red.** `RED_BANK` in `bringup.rs` is a guess. If the final result colours look inverted, flip it.
- **Polarity of the GNSS reset line** (PC6 through Q2), the CAN silent pin (PA10), the transceiver fault pins and the SD card-detect switches. The test reports the levels it reads.
- **Whether `$PAIR051` is answered by U10 on UART1 at the module's default baud.** The test tries four rates.
- **The LED open-load flags.** They are printed, not judged.

## The logger

```
cargo run --release                          flash and run
cargo run --release --features auto-prepare  also format cards that have no log region
```

Before it can log, each card needs a log region: prepare it with `tel-decode prepare` on the PC (see `../README.md`), or build with `--features auto-prepare` so the board formats a card the first time it sees it. **`auto-prepare` overwrites the first three blocks of any card the board does not recognise, which destroys the partition table of a card used for anything else.** It is off by default.

What it does, in order:

1. Checks the crystal (falls back to the internal oscillator and says so), starts the LED drivers, and starts both executors.
2. Each card task initialises its card and looks for a log region. The supervisor waits for both reports.
3. The GNSS task finds the baud rate the module is using (115200, 460800, 9600, 38400) and streams NMEA while it waits. When the supervisor asks, it checks the module settings (10 Hz, 460800 baud, `$PQTMTAR` on) and writes them if they differ, restarts the module through PC6 and verifies. If that fails, logging still starts and every sample is flagged `NO_FIX_START`.
4. When there is a fix (or after two minutes without one) the supervisor opens a session on each good card: a session header, a boot event, then records.
5. Records: one `Sample` per GNSS epoch (position, speed, heading, IMU window means and peaks, flags, counters), the newest SBG signals, raw CAN frames from both buses in batches, and events. Both cards get every record. Without GNSS data a sample is still written five times a second so the IMU and CAN are not lost.
6. The watchdog (3 s) is fed by the supervisor only while GNSS, IMU, the logger and the supervisor have all reported in recently.

Task layout:

| Executor | Tasks |
|---|---|
| High priority (interrupt `UART5`, below the peripheral interrupts) | PPS capture (TIM1), GNSS (UART4 with DMA ring buffer), IMU (SPI1, data-ready on PC4), CAN1, CAN2, SD1 (SDMMC1), supervisor with status LEDs and watchdog |
| Thread mode (lowest) | SD2 (SPI mode, blocking) |

SD2's driver blocks the CPU during a transfer, which is why it sits below everything else: the UART ring buffer, the CAN FIFOs and the watchdog all keep being served while it waits.

The numbers to confirm on the bench, all in `src/board.rs`: `CAN1_BITRATE` and `CAN2_BITRATE` (both 1 Mbit/s placeholders), `RED_BANK`, and `TAR_RATE`. The rover (U13) is not connected to the MCU's serial port: its heading reaches the MCU through U10, so `$PQTMTAR` is enabled on U10. Whether U10 really reports it on UART1 is unconfirmed until a module is on the bench; if the configuration step fails because of it, the logger still starts and flags the session.

Known limits of this version:

- The card-detect switches are not used; a card that is removed shows up as a failed write.
- The second GNSS module's PPS (PA3) is not captured; only U10's drives the clock.
- No USB console yet.
- CAN receive timestamps are taken when the frame is read, not from the controller's hardware timestamp.
- A full card ends the session and is reported as a card fault.

## What is in `src/`

- `src/board.rs`: clock setup and the board constants above, shared by both programs.
- `src/shared.rs`: the record queues, the supervisor event queue and the small amount of state tasks share.
- `src/sd.rs`: the two `BlockDevice` adapters the log uses.
  - `Sd1`: SDMMC1 4-bit (slot J3), over the `embassy-stm32` storage device.
  - `Sd2`: SPI mode (slot J11) over the `embedded-sdmmc` driver. The card must see at least 74 clocks at 400 kHz or less with chip select high before the first command, and the SPI clock can be raised after the card is initialised (`Sd2::with_spi`). Both programs do both steps.
- `src/bin/bringup.rs`: the self-test described above.
- `src/main.rs`: the logger.
