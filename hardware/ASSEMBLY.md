# Assembly and first power-up

How to hand-assemble the board in stages, with a check after each stage so a fault is found before it can damage anything else. Built from the netlist and PCB (`telemetry.net`, `telemetry.kicad_pcb`, 2026-10-13); designators match the BOM.

Statements marked **(unverified)** come from reasoning on the schematic or a datasheet and have not been confirmed on the real board. If a measurement disagrees with this guide, stop and investigate before continuing.

## Tools and supplies

- Soldering iron with a fine tip, plus **hot air or a hot plate** and **solder paste**. Several parts have no leads to reach with an iron (see below).
- Flux, fine solder wire (0.3 to 0.5 mm), flux remover, tweezers, a magnifier or microscope (the 0402 parts and the LGA need it).
- Multimeter. A **current-limited bench supply** is strongly preferred for the first 12 V power-up. A USB power meter helps for USB power.
- ESD precautions. Do not clean with solvent near the GNSS modules (see below).
- An SWD probe (ST-LINK or any probe probe-rs supports) and a 1x4 2.54 mm pin header for J12.

## What to fit and what to leave off

| Item | Decision |
|---|---|
| C28, C29, C49, C50 | **Leave empty.** Unused shunt pads of the antenna filters (marked DNP in the PCB). |
| R35, R42 | Fit as **0 ohm** links (series pads of the same filters). |
| J12 | Fit a 1x4 pin header (SWDIO, SWCLK, NRST, GND). It is DNP in the design files, so it is not in the BOM. It overlaps J2 on purpose: fit **J12 or the Tag-Connect**, never both. |
| J2 | A Tag-Connect footprint (pads only). Nothing to buy. |
| J9, J10 | Two SMA connectors joined to each other (RF test through-path). Optional; skip unless you want them. |
| H1 to H4 | Mounting holes; no component. |
| R10 | **73.2k**, not 63.4k (sets the over-voltage cut-off to 15.8 to 17.2 V). |

## The difficult parts

| Part | Package | Notes |
|---|---|---|
| U2 TPS2121RUXR | VQFN-HR-12, 2.5 x 2.0 mm, 0.5 mm pitch, pads underneath | Paste and hot air, or a stencil. Cannot be done with an iron. Check orientation twice. |
| U3 ASM330LHHX | LGA-14, 2.5 x 3.0 mm, pads underneath | Paste and hot air. Inspect for bridges by continuity afterwards. Keep it away from strong heat for long (MEMS part). |
| U1 LMR33610 | HSOP-8 with exposed pad and thermal vias | The pad must be soldered for heat and for ground. Paste and hot air, or flood the pad from the back through the vias. |
| U10, U13 LC29HEAMD | 12.2 x 16.0 x 2.5 mm LCC module | See the module rules below. Solder these **last**. |
| U6 STM32H523 | LQFP-64, 0.5 mm pitch | Drag soldering with flux works. Align pin 1 first. |
| U8, U9 TLC59108 | TSSOP-20 | Drag or reflow. |
| U12 TXS0104E | TSSOP-14 | Drag or reflow. |
| J6 USB-C | 16-pin SMD with shield tabs | Solder the shield tabs for strength. |

**GNSS module rules (Quectel LC29H hardware design V1.4):**
- The modules are MSL 3. Once the bag is opened, solder within **168 hours** (or within 24 hours if you are unsure of your storage conditions). If they sat open longer, bake **24 hours at 120 C** first and solder within 24 hours of the bake.
- The module may go through **one reflow cycle only**. Fit it after every other part that needs reflow, in a single pass.
- **Never wipe the shield can with solvent** (alcohol, acetone and similar rust it). No ultrasonic cleaning. Mask the modules when you clean flux off the board.
- Recommended solder-joint profile: peak 235 to 246 C, 40 to 70 s above 217 C.

**Polarity and orientation to check before soldering:** C6 (100 uF electrolytic), D1 (zener), D2 (TVS, SMBJ16A), D3 (Schottky), the five LEDs, Q1 and Q2 (SOT-23), and pin 1 of every IC and of the SD slots.

## Board sides

Everything is on the front except the second SD slot: **J11, R46 to R52 and C53 are on the back.** Solder the back first, with J11 last of the back parts so it is not heated again, then the front.

## Assembly stages

Do not move to the next stage until the checks pass.

### Stage 0: bare board
Check with the multimeter that +3.3V, +5V and V_IN are not shorted to GND (use TP2, TP1 and the C18 pad; TP4 is GND).

### Stage 1: power input
Fit: J1, F1 (polyfuse), D2 (TVS), Q1 and D1 and R7 (reverse-polarity switch), U2 with R1, R2, R3, R5, R6, R9, R10, R11, R12 and C2, D3, and the V_IN capacitors C3, C4, C5, C6, C17.

Checks (feed J1 from the bench supply, current limit 100 mA, start at 0 V and raise to 12 V):
- No short between V_IN (TP1) and GND at power-off.
- TP1 follows the input up to about 12 V minus the drop of Q1 and U2. Idle current is a few mA at most.
- Optional over-voltage test with the current limit still low: raise slowly above 15 V; V_IN should drop out between **15.8 and 17.2 V** (R10 = 73.2k). Do not go above 17 V with a supply that can deliver real current: the TVS D2 (SMBJ16A) starts to conduct around there. **(the cut-off range comes from the TPS2121 datasheet reference of 1.01 to 1.10 V)**
- Never connect the supply reversed to test the protection with an unlimited supply.

### Stage 2: 3.3 V buck
Fit: U1 (with its pad soldered), L1, C1, C12, R4, R8, and the 3.3 V output capacitors C7, C8, C9, C10.

Checks (12 V in, still current limited, nothing else fitted):
- **TP2 (+3V3) reads 3.3 V**, accept 3.2 to 3.4 V. The divider R4 100k and R8 43.2k with the datasheet's nominal 1 V reference gives 3.31 V.
- Supply current falls to a few mA (only the buck's quiescent current).
- **TP3 (PG_BUCK) is an open-drain pin with no pull-up on the board**, so a meter reads nothing meaningful there. Add a pull-up to TP2 temporarily if you want to see it go high.
- The buck inductor and IC should be cool.

### Stage 3: 5 V rail for the LEDs
Fit: U5 (LM2940, TO-263) and C18.

Check: the +5V rail (C18) reads 5.0 V with 12 V in. **(unverified)** On USB power only, V_IN is about 4.6 to 4.9 V, so +5V will read below 5 V; the LEDs run from constant-current sinks and are expected to still light.

### Stage 4: MCU core and SWD
Fit: U6, Y1, C15, C16, C22, C23, C24, SW3 (reset), SW4 (BOOT0), R34, the 3.3 V decoupling capacitors that sit beside U6 (the BOM groups them with the other 100 nF, 1 uF and 10 uF parts), J12 header, and the USB-C parts J6, R32, R33, U11 (and D3 from Stage 1).

Checks:
- 3.3 V current stays small (tens of mA) with the MCU in reset.
- Connect the probe to J12: pin 1 SWDIO, pin 2 SWCLK, pin 3 NRST, pin 4 GND. **J12 has no 3.3 V pin, so power the board from USB-C or J1.**
- `probe-rs info` (or your probe's tool) reads the chip ID. If it cannot, check Y1 soldering and the reset button first.
- Plug the USB-C into your PC: it should be powered (R32 and R33 present the 5.1k resistors a host needs). The PC will not see a USB device until firmware runs.

### Stage 5: LEDs
Fit: U8, U9, R30, R31, R24, R25, R26, LED1 to LED5.

Check: the bring-up app's LED test (below). Orientation of the LEDs is the likely mistake: confirm both colours light.

### Stage 6: IMU
Fit: U3 (LGA) and R20. Check: the bring-up app's IMU test (WHO_AM_I reads 0x6B).

### Stage 7: CAN
Fit: U4, U7, R21, R22, R23, R27, R28, R29, J4, J5, TP5 to TP8 if not already fitted. Check: the bring-up app's CAN tests. Each controller is tested alone by internal loopback. To test the transceivers and connectors, join J4 to J5 (CAN_H to CAN_H, CAN_L to CAN_L) with a 120 ohm resistor across the pair; frames must then cross in both directions. For a real bus you need a second node and 120 ohm termination (the board has none; confirm the car's bus is terminated).

### Stage 8: SD slots
Fit: J3 with R13 to R19; then (back side) J11 with R46 to R52 and C53. Check: the bring-up app's SD tests with a card inserted in each.

### Stage 9: GNSS
Fit: U12 (TXS0104E), C25, R36, Q2, R37, R38, then the antenna circuits for each module: R39 / R43 (10 ohm), L2 / L3 (56 nH), C27, C30, C31 and C48, C51, C52, D4, D5, R35, R42 (0 ohm links), then J7 and J8 (SMA). **Last of all, U10 and U13**, each in its single reflow pass.

Check: the bring-up app's GNSS test. Connect an active antenna (ANN-MB5) to J7 and J8 with a clear view of the sky for the fix test; indoors the modules talk but will not get a fix.

### Stage 10: finish
Any remaining 0402 decoupling capacitors from the BOM (place each beside the IC it serves), SW1 and SW2 (the 3-position switches), then inspect everything.

## Final inspection

- Look at every joint under magnification; re-flow anything dull or bridged.
- Check no solder bridges on U6, U8, U9, U12 pins.
- Remove flux residue (not near the GNSS modules' shield cans).
- Power from USB and confirm TP2 is 3.3 V before running the application.

## First flash

1. Probe on J12 as above; board powered from USB-C. Fit a card in each SD slot and connect the antennas.
2. In `software/firmware`, run `cargo run --release --bin bringup` (see `software/firmware/README.md` for what each check means and what it cannot know yet).
3. Watch the report printed by probe-rs (RTT) and the five status LEDs: green when every check in that group passed, red when one failed.
4. When it is clean, prepare two cards and flash the logger (`software/firmware/README.md`, section "The logger").
