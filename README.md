# ANUFS Telemetry

Data-logging and telemetry module for the ANUFS vehicle: an STM32H523 board that listens on two CAN buses, fuses dual-antenna GNSS and an IMU with them, and logs everything to two microSD cards.

| Folder | What is in it | Start here |
|---|---|---|
| [hardware/](hardware/) | KiCad project, fabrication files, BOM, power budget, RF analysis | [hardware/README.md](hardware/README.md) |
| [software/](software/) | Rust firmware crates, host tools and diagrams | [software/README.md](software/README.md) |

## Where things stand

- **Hardware:** boards are ordered and will be assembled by hand. Fit R10 as 73.2k (the BOM is up to date). Fit J12 as the SWD header until the Tag-Connect cable arrives.
- **Software:** every logic crate and host tool is written and tested on the PC (332 tests), and the firmware (a bring-up self-test and the logger) builds for the chip. Nothing has run on the board yet; assembly is in [hardware/ASSEMBLY.md](hardware/ASSEMBLY.md) and the bring-up order is in [software/README.md](software/README.md).
- **Waiting on you:** the MoTeC CAN configuration (bit rate and message layouts), and a debug probe.
