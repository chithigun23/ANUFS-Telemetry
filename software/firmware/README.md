# Firmware crate

The MCU-side crate. It is built separately from the PC workspace (it is listed under `exclude` in `../Cargo.toml`) because it targets the chip.

```
rustup target add thumbv8m.main-none-eabihf
cargo build --release
```

## What is here

- `src/sd.rs`: the two `BlockDevice` adapters the log uses.
  - `Sd1`: SDMMC1 4-bit with DMA (slot J3), over the `embassy-stm32` storage device.
  - `Sd2`: SPI mode (slot J11) over the `embedded-sdmmc` driver. That driver is blocking, so run SD2 at a low priority.
- `src/main.rs`: an entry point that initialises the HAL and idles. It proves everything links for the STM32H523RC (memory layout comes from the `embassy-stm32` `memory-x` feature).

Both adapters compile against the real HAL and are checked to implement `tel_store::BlockDevice`. They have not run on hardware.

## What is not here yet

The RTIC application that connects the tasks, written during bench bring-up because it needs the board to verify clocks (24 MHz crystal and PLL), interrupt priorities and DMA channels. All the logic it calls already exists and is tested in `../crates`:

| Task | Uses |
|---|---|
| GNSS parser | `tel-nmea`, `tel-core::clock`, `tel-core::fusion` |
| CAN receive | `tel-can`, `tel-can-sbg`, `tel-can-motec` |
| IMU | `tel-asm330`, `tel-core::imu_window` |
| Record builder and log writer | `tel-logfmt`, `tel-store` |
| Supervisor, LEDs, watchdog | `tel-core::supervisor`, `tel-tlc59108` |

Flashing needs a debug probe on J12 (or J2 with the Tag-Connect cable). The probe-rs chip name is probably `STM32H523RCTx`; confirm it with `probe-rs chip list` and then uncomment the `runner` line in `.cargo/config.toml`.
