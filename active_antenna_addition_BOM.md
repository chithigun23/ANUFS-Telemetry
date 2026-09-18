# Active Antenna Addition — Planning Doc

Status: **implemented in schematic** (telemetry.kicad_sch). Library symbols/footprints for
the new parts (LQW15AN56NG00D, PESD0402-140, DMG2305UX-7) exist in telemetry-library, though
the FET ended up unused — see below.

Goal, as built: dedicated active-antenna bias-tee on the existing SMA connector (J2), always
on (no switch — see "Design history" for why the switch was dropped, and why passive-antenna
compatibility is no longer a design goal: precise RTK positioning needs the active antenna's
phase-center stability and multipath rejection, so this connector is active-only going forward).

## Design history (superseded — kept for context)

Originally planned as two separate SMA connectors (existing J2 for passive, a new J601 for
active) selected by an RF switch (pSemi PE4259), so both antennas could be mounted
simultaneously with instant electronic selection. **Dropped this approach**: it only makes
sense if both antennas need to be physically connected at once.

Next iteration was a single connector supporting *either* antenna type, with bias switchable
via a FET (Q601/Q602/R602) so bias could be disabled for passive-antenna safety or power
saving. **Also dropped, for two reasons:**
1. Checked whether the module itself gates VDD_RF based on `ANT_ON` — it doesn't. VDD_RF just
   mirrors VCC always. And on this board, VCC is never power-cycled and `WAKEUP` is
   unconnected (no path to exit Backup mode), so the module never leaves Continuous mode in
   the first place — there's no power-saving event for a switch to respond to.
2. The actual goal is RTK positioning as close to 1cm as achievable. That needs the active
   antenna's phase-center stability and multipath rejection — a passive antenna is a strictly
   worse choice for this application, not a fallback worth keeping wired in. So "safely
   support either antenna type" stopped being a real requirement, and with it, the reason to
   worry about a passive antenna's DC-short fault mode on this connector went away too.

Net result: **no switch, active-only, bias always on.** `R601` (10Ω, in series with the bias
feed) still exists and still matters — it protects the module if the antenna connector is ever
shorted (e.g. a wiring fault, connector mishap), which is a real scenario worth guarding
against regardless of antenna type.

## Active antenna

**u-blox ANN-MB1-00**
- Dual-band: L1 (1559–1606 MHz) + L5/E5a/B2a/NavIC (1164–1188 MHz) — matches LC29H(EA)'s
  dual-band capability
- Bias: 3.0–5.0 V, typ. 15 mA @ 5V (VDD_RF on this board mirrors VCC = 3.3V, within spec)
- LNA gain: typ. 29 dB (L1) / 33 dB (L5), pre-filtered with an internal SAW
- 5 m RG174 cable, SMA connector, magnetic mount
- IP67, –40°C to +85°C, CE approved
- Source: [DigiKey](https://www.digikey.com/en/products/detail/u-blox/ANN-MB1-00/14835875)
- Datasheet: [ANN-MB1 Product Summary](https://content.u-blox.com/sites/default/files/ANN-MB1_ProductSummary_UBX-21000238.pdf)

## Topology (as built)

Follows Quectel's own active-antenna reference circuit (Hardware Design doc, Sheet 6), minus
the optional power switch. `VDD_RF → R40 → [C36, C38 decoupling] → L2 → antenna node [D2 TVS,
J2, C40 DC-block] → RF_IN`. Bias is always on.

| Schematic ref | Part | Value/Footprint | Connects |
|---|---|---|---|
| R40 | 10Ω | [Resistor_SMD:R_0402_1005Metric_Pad0.72x0.64mm_HandSolder](.) | VDD_RF (pin 9) → bias choke; limits fault current if the connector is ever shorted |
| C36 | 100nF | Capacitor_SMD:C_0402_1005Metric_Pad0.74x0.62mm_HandSolder | Decouple, on bias line near L2 |
| C38 | 100pF | Capacitor_SMD:C_0402_1005Metric_Pad0.74x0.62mm_HandSolder | Decouple, on bias line near L2 |
| L2 | Murata LQW15AN56NG00D, 56nH | telemetry:LQW15AN56NG00D, [DigiKey 490-6830-1-ND](https://www.digikey.com/en/products/detail/murata-electronics/LQW15AN56NG00D/3846027) | Feeds DC bias onto the RF_IN line without loading the RF signal. SRF 2.8GHz (56% margin above L1's 1575.42MHz — chosen over Quectel's literal 68nH spec, which only has ~3% SRF margin on any commonly available part), 200mA rating, $0.12 |
| D2 | Littelfuse PESD0402-140 | telemetry:PESD0402-140, [DigiKey PESD0402-140CT-ND](https://www.digikey.com/en/products/detail/littelfuse-inc/PESD0402-140/1968525) | ESD protection at the antenna node. 0.05pF (well under Quectel's <0.6pF recommendation), 14V standoff (clear of the 3.3V bias), $0.80 |
| C40 | 100pF | Capacitor_SMD:C_0402_1005Metric_Pad0.74x0.62mm_HandSolder | DC block between the antenna node and RF_IN (pin 11) |
| J2 | (existing) | — | Antenna connector, SMA |

**Note:** `R40`'s footprint was originally set to `Resistor_THT:R_Array_SIP4` (wrong package,
clearly a leftover default) and `C36`/`C38`/`C40` had no footprint at all — all four fixed
directly in the schematic (2026-09-18).

**Sourcing note:** AEC-Q200 was not treated as a requirement when picking L2/D2 —
for a Formula Student car (rebuilt/inspected each season, not a 10+ year OEM warranty part),
the real drivers are electrical fit and physical robustness (temp range, small rigid SMD
construction), not automotive PPAP qualification. Both parts are confirmed direct
DigiKey stock, not DigiKey Marketplace listings.

**Not used: Diodes Inc DMG2305UX-7 P-FET.** A symbol/footprint (`telemetry:DMG2305UX-7`)
exists in the library from when a bias switch was still planned, but the switch was dropped
(see Design history) before it got used anywhere in the schematic. The board does have an
unrelated `Q1` (DMP3099LQ-7) and `Q2` (MMBT3904) — those belong to a separate 12V input
protection circuit, not this antenna feature; don't confuse the two when reading the BOM.

## Bandpass filtering — decided against, not implementing

Considered adding a board-level bandpass filter for automotive EMI robustness (Quectel's
reference design marks this optional). **Decision: not using an external bandpass filter.**
The single-connector switchable-bias feature above stands on its own without it.

Research is kept below for reference in case this gets revisited later — e.g. if EMI proves
to be a real problem once the car is running and generating RF noise on track.

<details>
<summary>Bandpass filter research (not currently planned)</summary>

LC29H(EA) has a single dual-band `RF_IN` pin (no separate L1/L2 inputs), so what would be
needed is a filter that passes *both* L1 and L5 on one line while rejecting everything else.

**Rejected option — Taoglas DXP.02.A SAW diplexer used as a combiner.** Considered splitting
the antenna signal 2 ways, feeding one leg into the diplexer's L1 port and the other into its
L2/L5 port, and taking the recombined signal off Common. **Do not use this** — a diplexer's
ports are only 50Ω-matched *within that port's own passband*; band separation works by
reflecting out-of-band energy, not absorbing it. Feeding a dual-band signal into a single-band
port means the other band's energy hits an uncharacterized, reflective mismatch at that port.
Combined with ~3dB splitter loss and ~4dB diplexer loss, this is not a clean 50Ω system.

**If revisited, use instead — Abracon AFII-LC-0081 full-band GNSS ceramic bandpass filter:**
- Genuine 2-port device (Input/Output), not a diplexer
- 50Ω source and load impedance, both ports — datasheet states "a matching network is
  unnecessary." VSWR 1.2 typ / 1.5 max across the entire 1125–1675 MHz passband, so both L1
  and L5 sit cleanly inside one continuous, well-matched band
- Insertion loss: 0.85dB typ / 1.9dB max
- Rejection: 40dB below 900MHz, 25dB at 900-1002MHz, 35dB at 2-2.5GHz, 27dB at 2.5-5.95GHz
- Has internal DC-blocking caps on both input/output — would need to sit upstream of the
  bias-injection point on the active leg
- 3.2×2.5×1.75mm SMD, -45°C to +85°C, MSL1, automotive-validated COTS per Abracon's datasheet
- ~$0.76. [DigiKey (AFII-LC-0081-T)](https://www.digikey.com/en/products/detail/abracon-llc/AFII-LC-0081-T/) · [Datasheet](https://abracon.com/datasheets/AFII-LC-0081.pdf)

</details>
