# Telemetry software

Rust, `no_std`, bare-metal software for the STM32H523RCT7 on the ANUFS telemetry board, plus the host tools that read its logs. Hardware reference: [../hardware/README.md](../hardware/README.md).

| Diagram | What it shows |
|---|---|
| [build-vs-reuse.svg](docs/build-vs-reuse.svg) | What was reused, what was written, and where the plan changed |
| [software-layers.svg](docs/software-layers.svg) | Crate and layer structure, from the firmware programs down to the chip |
| [firmware-architecture.svg](docs/firmware-architecture.svg) | Data flow from pins to SD card, with interrupt priorities |
| [firmware-flow.svg](docs/firmware-flow.svg) | Boot sequence, run loop and failure handling |

## Status

Every crate and tool in the table, except `firmware/`, is finished and tested on the PC, and the `no_std` crates build for the Cortex-M33 (`thumbv8m.main-none-eabihf`). `firmware/` holds two programs, the bring-up self-test and the logger, which build and link for the chip. **None of it has run on the board yet.** 332 tests pass; every logic crate is `no_std`, `forbid(unsafe_code)`, and clean under clippy with `unwrap`, `expect`, `panic`, indexing and unchecked arithmetic denied.

| Crate | What it does | Tests |
|---|---|---|
| `tel-nmea` | NMEA framing and parsing (RMC, GGA, `$PQTMTAR`), Quectel PAIR/PQTM command builder, GNSS configure-and-verify sequencer | 69 |
| `tel-can` | Bounds-checked CAN signal extraction (both byte orders), message catalog, latest-value store | 26 |
| `tel-can-sbg` | SBG Ellipse messages, generated from `sbgCan.dbc`; resolved ids for the signals we log | 9 |
| `tel-can-motec` | MoTeC messages, generated from `config/motec.csv` (empty until the real config is supplied) | 3 |
| `tel-asm330` | ASM330LHHX SPI driver with read-back verified configuration | 18 |
| `tel-tlc59108` | LED driver and the two-chip five-LED bank | 20 |
| `tel-logfmt` | Crash-safe append-only block log: CRC blocks, records, sans-IO writer, recovery | 48 |
| `tel-core` | Timer extension, PPS-disciplined UTC clock, IMU window, GNSS/IMU fusion into samples, supervisor | 66 |
| `tel-store` | Async storage driver over a `BlockDevice` trait: prepare, recover, write service | 7 |
| `tel-selftest` | Pass/fail rules and report for the bring-up self-test (IMU, GNSS, PPS, CAN, SD, LED drivers, clock against PPS) | 27 |
| `tools/dbc2table` | DBC or CSV to Rust message tables | 24 |
| `tools/tel-decode` | Host tool: scan, report, CSV export, prepare; also holds the whole-system test | 15 |
| `firmware/` | `bringup` (checks every device on the board) and the logger, plus the SD1 and SD2 `BlockDevice` adapters | builds only |

The system test (`tools/tel-decode/tests/pipeline.rs`) runs a scripted 30 second drive through the real code from NMEA bytes to card: noisy GNSS stream, PPS timestamps, SBG CAN frames, IMU samples, supervisor, two cards with one dying half way. It then reads both cards back with the host tool and checks positions, UTC to within 1 ms, error counters, decoded signals and that the dead card holds a clean prefix of the good one.

How the tests were hardened: the parsing, driver, log and core crates were mutation-checked by hand (break one line, confirm a test fails), and where a mutation survived a test was added. `tel-logfmt` also survived 5000 randomised power-loss scenarios with torn writes.

## Run it

```
cargo test --workspace                       all tests
cargo clippy --workspace --all-targets       lints
cargo build -p tel-core --release --target thumbv8m.main-none-eabihf     any logic crate, for the MCU
cd firmware && cargo build --release         both firmware programs (needs the thumbv8m.main-none-eabihf target)
cd firmware && cargo run --release --bin bringup      flash the self-test through the probe and print the report
cd firmware && cargo run --release                    flash the logger
cargo run -p tel-decode -- info card.img     report the sessions on a card image
cargo run -p tel-decode -- samples card.img --session 1 -o drive.csv
```

Regenerate the CAN tables after changing a DBC or the CSV (the tests fail if a checked-in table is stale):

```
cargo run -p dbc2table -- crates/tel-can-sbg/dbc/sbgCan.dbc crates/tel-can-sbg/src/tables.rs --skip-file crates/tel-can-sbg/dbc/skip.txt
cargo run -p dbc2table -- crates/tel-can-motec/config/motec.csv crates/tel-can-motec/src/tables.rs
```

## Decisions

| Topic | Decision | Why |
|---|---|---|
| Language | Rust | Memory safety, host-testable logic |
| Execution model | Bare-metal, two `embassy-executor` executors: a high priority interrupt executor for everything time-critical (and the supervisor and watchdog), and thread mode for the blocking SD2 driver | No RTOS, static priorities, and the HAL's async drivers are built for this executor. RTIC was the first plan; it was dropped because the HAL's async drivers and time driver are built around the embassy executor, and mixing the two was not tried |
| Peripheral drivers | `embassy-stm32` 0.6 | Has FDCAN, SDMMC, SPI, I2C, UART, USB and input-capture drivers for this chip |
| Time base | `embassy-time` at 1 MHz on TIM12; PPS stamped by the TIM1 input capture | The capture age is subtracted from the monotonic clock, so interrupt latency does not matter |
| Target | `thumbv8m.main-none-eabihf` | Cortex-M33 with FPU |
| Programming | probe-rs over SWD (J12 header until the TC2030 cable arrives) | probe-rs lists STM32H523RC as a target |
| Record clock | GNSS fix rate, 10 Hz | LC29H(EA) maximum, 100 ms is also its default |
| On-card format | **Raw 512 byte blocks, not FAT** | See below |
| Cards | Both slots logged to at once (mirror) | Losing one card does not interrupt logging |
| CAN | Both buses in restricted operation mode (acknowledge, never transmit), plus a raw frame lane | The Ellipse needs a listener to acknowledge it; the real MoTeC layouts are unknown, so raw frames let them be decoded later |
| Card preparation | A card must be prepared with `tel-decode prepare` first; the `auto-prepare` cargo feature lets the board do it | Preparing overwrites the first blocks of the card, so it is never automatic |

### Why raw blocks instead of FAT

A race car loses power without warning. FAT must update directory and allocation tables to make data visible, so a power cut can lose a whole file or corrupt the volume. The log instead writes each 512 byte block once, never rewrites it, and puts a CRC-32 in it, so a power cut can damage only the block being written and the CRC always shows which. Recovery needs only a few blocks of scanning thanks to two alternating checkpoint ("hint") blocks. The cost: the card is not a normal drive.

- A prepared card shows as **unformatted in Windows. Do not click Format.** Read it with `tel-decode`, which opens the card or an image of it.
- Reading a physical card on Windows needs an administrator terminal and a path such as `\\.\PhysicalDrive2`. Safer: make an image first, then run `tel-decode` on the image.
- `prepare` overwrites the first three blocks of the region. With `--start-lba 0` that is the partition table. To keep a FAT partition for other uses, start the region after it.

## What was verified against documents

- `embassy-stm32` 0.6.0 has a `stm32h523rc` feature; FDCAN, SDMMC, SPI, I2C, UART, USB and input-capture drivers all compile for it.
- Every MCU pin maps to the intended peripheral in the STM32H523 datasheet (DS14540 Rev 3). SD2 is SPI2 (PB2 SCK, PB14 MISO, PB15 MOSI, PB0 CS) because the H523 has one SDMMC.
- Register maps, bit encodings and sensitivities for the IMU (DS13197 Rev 9) and the LED driver (SLDS156B) were read from the datasheets, not recalled.
- Quectel command checksums in the tests are the ones printed in the Quectel documents.
- NMEA, PQTMTAR and PAIR formats follow the LC29H protocol specification V1.5 and the Moving Base note V1.1.
- Reference timestamps and CRC check values were generated independently with Python.

## Things found while building

- The `asm330lhhx-rs` crate depends on a yanked release and does not build, so the IMU driver was written.
- The TLC59108 powers up with its oscillator **off** and responding to a broadcast address; the driver turns the oscillator on and disables the broadcast addresses. Its I2C address is `1 0 0 A3 A2 A1 A0`, so U8 is 0x40 and U9 is 0x41.
- SBG's `sbgCan.dbc` has one malformed range and two messages (`GPS1_POS_SECURITY_STATUS`, `GPS2_POS_SECURITY_STATUS`) whose signal does not fit the declared length. The generator tolerates the first, and refuses the second unless the message is listed in `crates/tel-can-sbg/dbc/skip.txt` (they are, and the logger does not use them).
- A failed `flush()` or `close()` because the write queue is full is normal and must be retried after servicing the card; the host tests exercise this.
- The firmware must treat a failed read during start-up recovery as an error, not as the end of the data, or it could overwrite valid blocks. Recovery does.

## Not verified yet (needs the board or real data)

- Anything on real hardware: clocks, interrupt priorities, DMA channels, the executor split, whether the UART ring buffer survives a blocking SD2 write.
- SDMMC1 behaviour at speed and SPI-mode SD (SD2), including SD2's blocking transfers.
- Which LED pin is red and which is green (`ColorMap::ASSUMED` is a guess; `StatusLeds::light_bank` identifies it).
- The GNSS configure sequence order against a real LC29H(EA), and which module receives the `$PQTMTAR` enable command.
- MoTeC CAN bit rate and message layouts (fill in `config/motec.csv`), and the Ellipse-D output configuration.

## Memory safety, honestly

- Logic crates use `#![forbid(unsafe_code)]` and are checked by clippy with panics and unchecked arithmetic denied.
- `unsafe` still exists in the PAC, HAL, `cortex-m` and the code the embassy macros generate. That is unavoidable for any Rust firmware. The firmware crate itself has one `unsafe` function, the executor's interrupt hook, and `#![deny(unsafe_code)]` everywhere else.
- No heap. Every buffer and queue has a size fixed at compile time.
- The hardware adapters in `firmware/src/sd.rs` are thin and contain no `unsafe` of their own.

## Bring-up order

Assemble in stages (`../hardware/ASSEMBLY.md`), then:

1. Flash `bringup` (`firmware/README.md`). It checks the crystal, LED drivers, IMU, both CAN controllers, both GNSS modules and PPS, and both SD slots, and prints one line per check. Fix whatever it reports before going on.
2. Look at the LED colour test and set `RED_BANK` in `firmware/src/board.rs` if red and green are swapped.
3. Prepare two cards with `tel-decode prepare` (or build with `--features auto-prepare`).
4. Flash the logger. Watch the RTT log and the LEDs: cards mount, GNSS is configured, a fix arrives, sessions open.
5. Bench test on a CAN bus, then on the car. Fill in `config/motec.csv` from the MoTeC configuration and regenerate the tables.
6. Power-cycle test: cut power during logging, then read the card with `tel-decode` and check the log ends cleanly at a block boundary.
7. Soak test with both cards writing while watching the GNSS C/N0.

## Open questions

- Which debug probe do you have?
- Real CAN bit rates and DBC or MoTeC CAN configs, when you have them.
- Whether the Ellipse-D or the onboard GNSS and IMU is the reference for analysis.
