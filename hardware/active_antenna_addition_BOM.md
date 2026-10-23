# GNSS Antennas and Heading: Design Doc

Status: **implemented in schematic and PCB** (netlist dated 2026-10-13). Designators below match `telemetry.net`. An older revision of this doc used different designators (J2/J8, R40/R41, U15, D2); those are obsolete.

Two features:
1. Active-antenna bias-tees on two SMA connectors (J7 for U10, J8 for U13).
2. Dual-antenna heading via moving-base RTK between two LC29H(EA) modules, wired per Quectel's *LC29H(EA) Moving Base Application Note V1.1*.

## Roles

| Module | Role | Connection to MCU |
|---|---|---|
| U10 LC29HEAMD | **Base**. Outputs position NMEA and, per Quectel's note, the heading computed by the rover | UART1 (pins 20/21) through U12 (TXS0104EPW) to STM32 UART4 on PA1/PA0. 1PPS (pin 3) to PA9 through U12 |
| U13 LC29HEAMD | **Rover**. Computes baseline and heading | No data link to the MCU. 1PPS (pin 3) to PA3 through U12 |

Both modules run from +3.3V. U10's `VDD_EXT_1` is the logic reference for U12's A side. U13's `VDD_EXT_2` (pin 7) is not connected to anything else.

### Module-to-module link (per Quectel moving-base note)

| From | To | Net |
|---|---|---|
| U10 TXD2, pin 18 | U13 RXD1, pin 21 | `CORRECTION` |
| U13 TXD1, pin 20 | U10 RXD2, pin 19 | `HEADING` |
| U10 RESET_CTL, pin 17 | U13 RESET_N, pin 8 | `GNSS_RESET_SLAVE` |

Pins 5, 6, 15, 16 on both modules are RESERVED and left unconnected. WAKEUP (pin 1) is unconnected, so the modules never leave continuous mode.

**Reset:** MCU `PC6` (`GNSS_RESET_ASSERT`) drives Q2 (MMBT3904) through R37/R38, which pulls U10 RESET_N low. U10 then resets U13 through RESET_CTL. This was a shared reset net in an earlier revision.

## Antennas

**Currently planned: u-blox ANN-MB5** (three being bought).
- Bands: L1 (1559 to 1608 MHz) and L5/E5a/B2a/NavIC (1164 to 1186 MHz), matching LC29H(EA).
- Supply 3 to 5 V, 17 mA. The board's 3.3 V `VDD_RF` is inside that range.
- LNA gain typ. 15 dB (L1) and 16 dB (L5), 3 m cable with SMA.
- Source: [ANN-MB5 datasheet](https://cdn.sparkfun.com/assets/0/b/a/5/2/ANN-MB5_Datasheet.pdf)

**Why not the ANN-MB1-00 used in the earlier revision of this doc:** Quectel's *LC29H Series Hardware Design V1.4* (Table 9, recommended antenna specifications) gives "Active Antenna Total Gain: < 17 dB", where total gain is the internal LNA gain minus the total insertion loss of cables and components inside the antenna. The ANN-MB1's 29 dB (L1) and 33 dB (L5) LNA gain is well above that unless cable and component losses are about 12 to 16 dB. The ANN-MB5's 15 to 16 dB LNA gain is inside the limit before any cable loss. The same table asks for noise figure < 1.5 dB, out-of-band rejection > 30 dB, and for RTK, phase center offset and variation < 20 mm each.

**Heading baseline:** minimum antenna separation for reliable RTK-Fixed heading is realistically 30 cm or more. Quectel's documented minimum is 0.2 m, but field reports suggest 27 cm baselines only reach RTK-Float. Mounting on the car must allow this.

## Bias-tee topology (as built, one per module)

Follows Quectel's active-antenna reference circuit (Hardware Design, active antenna reference design), without the optional power switch. Bias is always on.

`VDD_RF -> R (10 ohm) -> [100pF, 100nF decoupling] -> L (56nH) -> antenna node [TVS, SMA, DC-block 100pF] -> pi pads -> RF_IN`

| Function | U10 (J7) | U13 (J8) | Part |
|---|---|---|---|
| Bias fault-current limiter | R39, 10 ohm | R43, 10 ohm | 0402 resistor. Protects the module if the connector is shorted |
| Bias decoupling | C30 (100pF C0G), C31 (100nF X7R) | C51 (100pF C0G), C52 (100nF X7R) | See sourcing below |
| Bias choke | L2 | L3 | Murata LQW15AN56NG00D, 56nH, [DigiKey 490-6830-1-ND](https://www.digikey.com/en/products/detail/murata-electronics/LQW15AN56NG00D/3846027), SRF 2.8 GHz |
| Antenna-node ESD | D4 | D5 | Littelfuse PESD0402-140, [DigiKey PESD0402-140CT-ND](https://www.digikey.com/en/products/detail/littelfuse-inc/PESD0402-140/1968525), 0.05 pF, 14 V standoff |
| DC block | C27 (100pF C0G) | C48 (100pF C0G) | Yageo CC0402JRNPO9BN101, [LCSC C106200](https://www.lcsc.com/product-detail/Multilayer-Ceramic-Capacitors-MLCC-SMD-SMT_YAGEO-CC0402JRNPO9BN101_C106200.html), 50 V |
| Pi-filter pads | C28 (shunt, empty), R35 (series, 0 ohm), C29 (shunt, empty) | C49 (shunt, empty), R42 (series, 0 ohm), C50 (shunt, empty) | Empty shunt pads were left unpopulated on purpose |
| Antenna connector | J7 | J8 | GCT RFPC-SMA27-F, right-angle THT SMA jack, [DigiKey 22162144](https://www.digikey.com/en/products/detail/gct/RFPC-SMA27-F/22162144). Had backorder/out-of-stock status at DigiKey when last checked, so check before ordering |

- 100nF X7R is used for the shunt bypass (100nF is not practically available in C0G at 0402). The part chosen in the earlier revision was Yageo CC0402KRX7R7BB104, [LCSC C60474](https://www.lcsc.com/product-detail/C60474.html). Confirm the BOM still uses it.
- C0G/NP0 is used for the DC block because it sits directly in the RF path. X7R's dielectric loss would add insertion loss there.
- L2/L3 are 56nH instead of Quectel's literal 68nH: the 68nH parts commonly available have only about 3% SRF margin over L1 (1575.42 MHz), while this part has about 56%.
- `ANT_ON` (pin 14) on both modules is not connected. `VDD_RF` mirrors VCC and the module does not gate it.
- AEC-Q200 was not required for L2/D4/D5. For a Formula Student car the drivers are electrical fit and physical robustness.

### RF test through-path (J9, J10)

J9 and J10 (also RFPC-SMA27-F) are joined only to each other on net `ANT3`. They are not connected to a module. See `RF Analysis/` for the simulation work that motivated the 0.36 mm trace width.

## Design history (superseded)

- An RF switch (pSemi PE4259) to choose between a passive and an active antenna was dropped: it only makes sense if both antennas are connected at once.
- A switchable bias (Diodes DMG2305UX-7 P-FET) was dropped because the module does not gate `VDD_RF` on `ANT_ON`, and because RTK to about 1 cm needs the active antenna's phase-center stability and multipath rejection, so a passive antenna is not a fallback worth wiring in. The `telemetry:DMG2305UX-7` library symbol is unused.
- A second MCU-facing level shifter and shared UART2 link between the modules (earlier revisions) were removed. The current wiring follows Quectel's moving-base note.
- Note: `Q1` (DMP3099LQ-7) is the 12 V reverse-polarity P-FET. `Q2` (MMBT3904) is the GNSS reset transistor. They are unrelated to the antenna bias.

## Firmware TODO

- U10 and U13 identify themselves as base and rover automatically when wired as above (per the moving-base note). No special configuration is required for the pairing.
- Enable `$PQTMTAR` with `$PQTMCFGMSGRATE,W,PQTMTAR,1,1` as in the note. The note says the message is produced by the rover and output by the base. **Confirm on the bench** which module receives the enable command and that `$PQTMTAR` appears on U10's UART1.
- Position fix interval is `$PAIR050` (100 to 1000 ms, default 100 ms for LC29H(EA)). Above 1 Hz only RMC and GGA output at the fast rate. At 10 Hz use UART baud 460800 or higher (`$PAIR864`); the protocol spec warns of lost messages below that.
- 10 Hz has been reported in the field to give worse RTK-Fixed stability than 1 to 5 Hz. Make the rate configurable and test both.
- STM32 side: UART4 (AF8) on PA0/PA1 reads U10's NMEA and `$PQTMTAR`. See [../software/README.md](../software/README.md).

## Bandpass filtering: decided against, not implementing

Considered a board-level bandpass filter for automotive EMI robustness (Quectel's reference design marks it optional). **Decision: no external bandpass filter.** Research is kept for reference in case EMI proves to be a problem once the car is running.

<details>
<summary>Bandpass filter research (not currently planned)</summary>

LC29H(EA) has a single dual-band `RF_IN` pin (no separate L1/L5 inputs), so a filter must pass both L1 and L5 on one line while rejecting everything else.

**Rejected: Taoglas DXP.02.A SAW diplexer used as a combiner.** A diplexer's ports are only 50 ohm matched within that port's own passband. Feeding a dual-band signal into a single-band port leaves the other band's energy hitting an uncharacterized, reflective mismatch. Together with about 3 dB splitter loss and about 4 dB diplexer loss, this is not a clean 50 ohm system.

**If revisited, use: Abracon AFII-LC-0081 full-band GNSS ceramic bandpass filter.**
- Genuine 2-port device, 50 ohm source and load, no matching network needed. VSWR 1.2 typ / 1.5 max over 1125 to 1675 MHz, so L1 and L5 sit inside one continuous matched band.
- Insertion loss 0.85 dB typ / 1.9 dB max.
- Rejection: 40 dB below 900 MHz, 25 dB at 900 to 1002 MHz, 35 dB at 2 to 2.5 GHz, 27 dB at 2.5 to 5.95 GHz.
- Internal DC-blocking capacitors on both ports, so it would sit upstream of the bias-injection point.
- 3.2 x 2.5 x 1.75 mm SMD, -45 to +85 C, MSL1.
- About $0.76. [DigiKey (AFII-LC-0081-T)](https://www.digikey.com/en/products/detail/abracon-llc/AFII-LC-0081-T/) and [datasheet](https://abracon.com/datasheets/AFII-LC-0081.pdf)

</details>
