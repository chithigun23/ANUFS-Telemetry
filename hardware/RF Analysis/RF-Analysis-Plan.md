# RF Analysis Plan — GNSS Antenna Path (Reflections from Filter 

## 1. The problem

Each GNSS receiver (`U10` for ANT1, `U13` for ANT2) is fed from an SMA connector through a chain that
carries three jobs on one 50 Ω line:

1. **RF signal path** to the module `RF_IN` pin.
2. **Bias-tee**: DC bias for the active antenna (u-blox ANN-MB1) is injected through a 56 nH inductor
   (`L2` / `L3`) with decoupling capacitors and a series protection resistor on the bias side.
3. **Protection and tuning**: a TVS diode (`D4` / `D5`), a series DC-block capacitor, and a pi filter.

Every one of these parts hangs something off the RF line: a stub, a shunt capacitance, a pad wider than
the trace, or a via to ground. Each is a small impedance discontinuity. They each reflect part
of the signal back toward the antenna, the point of this analysis is to find out if thats meaningful.

#### Focus of study
**What we are worried about:** the combined reflections degrade the signal at `RF_IN`, either through attenuation loss or through ripple in the passband from reflections bouncing
between the discontinuities. For an RTK receiver aiming at centimetre-level accuracy on L1 and L5, we
want to know this before fabrication. Gains that we make in layout will aim to improve the accuracy of the RTK solution.

#### System Boundries
**What we are *not* asking:** whether the antenna itself, including the wire and SMA plug, performs well. That is the vendor's job, external consideration.
This analysis starts at the GNSS module and stops at the SMA connector.

## 2. Components of the Board

### 2.1 Chains

| Chain | Connector | Line net | Bias tap | TVS | DC block | Pi filter (shunt / series / shunt) | Receiver |
|---|---|---|---|---|---|---|---|
| ANT1 | `J7` | `ANT1` | `L2` 56 nH, `C30` 100 pF, `C31` 100 nF | `D4` | `C27` 100 pF | `C28` / `R35` 0 Ω / `C29` | `U10` `RF_IN_1` |
| ANT2 | `J8` | `ANT2` | `L3` 56 nH, `C51` 100 pF, `C52` 100 nF | `D5` | `C48` 100 pF | `C49` / `R42` 0 Ω / `C50` | `U13` `RF_IN_2` |
| ANT3 | `J9`, `J10` | `ANT3` | none | none | none | none | test structure (see 2.3) |

`C28`, `C29`, `C49`, `C50` are unpopulated capcitor footprints, they could be used for tuning. The simulation would consider them as empty pads. It must be acknoledged that if capacitors were added for tuning purposes, this would alter the EMC simulation. This may be considered at a later point. 

### 2.2 Geometry and stackup
The following stack up is extracted using AI from the KiCad files, this alignes with the default 4 layer board avaiable from JLCPCB. 

| Item | Value | Source |
|---|---|---|
| Stackup | 4 layer: F.Cu / 0.2104 mm prepreg / In1.Cu / 1.065 mm core / In2.Cu / 0.2104 mm prepreg / B.Cu | PCB stackup; matches JLCPCB JLC04161H-7628 |
| Dielectric | Prepreg 7628: εr 4.4; core: εr 4.6; tan δ 0.02 | PCB stackup, updated to [JLCPCB's published values](https://jlcpcb.com/impedance). tan δ is not published by JLCPCB, so 0.02 is a generic FR4 assumption |
| RF trace width | 0.32 mm on F.Cu | PCB tracks |
| Reference plane | `GND` pour on F.Cu, In1.Cu and B.Cu (pour named "Ground Pour"); In2 is the 3.3 V power pour | PCB zones |
| Trace lengths | ANT1 line 9.2 mm; RF_IN_1 3.5 mm; between C27 and R35 2.4 mm | PCB tracks |
| Bias-side trace | 0.2 mm (ANT2 bias node, `Net-(C51-Pad1)`) — DC only | PCB tracks |

Microstrip impedance (estimate): With a 0.32 mm trace over 0.2104 mm of prepreg at εr 4.4, the standard Hammerstad-Jensen formula gives about 54 Ω. The formula assumes the only ground is the plane beneath the trace. It ignores the GND pour on F.Cu next to the trace, which lowers the real impedance. The 54 Ω is an upper bound. A width of roughly 0.36 mm would give 50 Ω on this stackup, the current width will be kept for now due to ground plane cleance reducing the impedance.






### 2.3 The ANT3 structure

`J9` and `J10` both connect to net `ANT3` through a 10 mm 0.32 mm line, placed to look like a
through-line. This is the intended for calibration. These connectors will be used as an experimental verification of this analysis.

## 3. First-principles estimate

Wavelength in the line at the GNSS bands (effective εr about 3.3 *(estimate)*):

| Band | Frequency | Wavelength on this line |
|---|---|---|
| L5 / E5a | 1176 MHz | about 140 mm |
| L1 | 1575 MHz | about 105 mm |

Every segment on the chain is 2–10 mm, well under λ/10 (about 10 mm). So each discontinuity behaves
like a **lumped parasitic**, not a transmission-line reflection. 

The reflections do not
build up standing waves along the trace. It also means the answer depends on parasitic values that only a
simulation or measurement will show us.

Return loss (RL) from single shunt elements on a 50 Ω line *(estimate, ideal parts)*:

| Element on the RF line | RL at L5 | RL at L1 | Comment |
|---|---|---|---|
| 56 nH bias inductor (ideal) | 24 dB | 27 dB | fine; parasitic capacitance near its 2.8 GHz SRF makes it worse |
| 0.05 pF (TVS, per its datasheet) | 41 dB | 38 dB | negligible on its own |
| 0.2 pF (a pad or small stub) | 29 dB | 26 dB | fine |
| 0.5 pF (a pad plus a via stub) | 21 dB | 18 dB | still acceptable |

The individual parts are each likely fine. The concern is **the sum**, in particular:

- the tap for `L2` sitting on the RF line as a stub,
- pad-width steps where the 0.32 mm trace meets 0402 pads,
- the unpopulated pi-filter shunt pads,
- ground vias near each shunt element (via inductance),
- the connector launch at `J7`/`J8`,

interacting with each other, since discontinuities spaced closer than λ/10 can partly cancel or add.

**How much a mismatch matters here:** the active antenna's LNA (typ. 29 dB gain on L1) sits *before*
this chain, so a small loss after the LNA barely moves the system noise figure. A 10 dB return loss
costs about 0.46 dB in delivered power. What we mainly want to avoid is (a) a resonance or notch inside
the L1/L5 bands and (b) large group-delay variation, since RTK carrier-phase depends on stable phase.
That drives the acceptance criteria below.

## 4. Hypotheses to test

Each is a specific, testable statement about the layout.

| # | Hypothesis | Test |
|---|---|---|
| H1 | The `L2`/`L3` tap stub and its pad add enough shunt capacitance to give under 15 dB RL in band | Step 3 |
| H2 | The TVS (`D4`/`D5`) pad and its ground via add more capacitance than the datasheet 0.05 pF | Step 3 |
| H3 | Pad-width steps at the 0402 DC-block and 0 Ω parts produce a measurable impedance bump | Step 3 |
| H4 | Empty pi-filter shunt pads (`C28`/`C29`/`C49`/`C50`) add stub capacitance | Step 4 |
| H5 | The 54 Ω trace (vs 50 Ω) plus the coplanar pour leaves the line impedance off target | Step 1 |
| H6 | The J7/J8 SMA launch is a larger discontinuity than the filter parts | Step 2 |
| H7 | The whole chain has an in-band notch or ripple from interacting discontinuities | Step 5 |
| H8 | Bias-network RF leakage: RF energy couples into the VDD_RF supply through the tap | Step 5 |

## 5. Tools

**Primary: openEMS** (open-source FDTD 3D field solver), driven from KiCad.

| Tool | Role |
|---|---|
| [KiCad-RFsim](https://github.com/RolandWa/kicad-rfsim) | KiCad 10 plugin: click pads for ports, runs openEMS, returns S-parameters. First choice. Very new, so validate it (Step 1). |
| [Gerber2EMS](https://github.com/antmicro/gerber2ems) | Fallback if the plugin is not reliable: Gerbers to openEMS with automatic mesh. |
| Qucs-S or scikit-rf | Cross-check with a lumped/circuit model of the chain built from vendor S-parameter files. |
| NanoVNA (or better VNA) | Ground truth on the real board (Step 6), using the ANT3 coupon and the populated chains. |

Vendor S-parameter (Touchstone) files to collect for the circuit model and for the simulation's lumped
elements: Murata LQW15AN56NG00D (SimSurfing), the C0G 100 pF parts, and Littelfuse PESD0402-140.

**Setup notes (from the plugin's README):** openEMS Windows build in `C:\openEMS`, a Python 3.14 venv
with CSXCAD/openEMS, and scikit-rf/matplotlib/h5py in KiCad's Python. The plugin's coarse mesh preset
under-reads line impedance (47.7 Ω vs 49.8 Ω theoretical), so use a finer mesh.

The software setup and running of the simulation will predominanty use Claude Sonnet 5 Medium in agentic mode. Other isntances of agenetic AI will be used to test and verify the methatology used for RF analysis.  

## 6. Method

The approach is **incremental**: start with a bare line, then add one feature at a time and record the
S-parameters after each. The change between steps shows which feature causes which problem. 

The the whole chain will not be simulated first in one go. This allows for the identification of modelling failures. 

Only the RF region is simulated (connector to `RF_IN` pad, cropped board), not the whole PCB.

An extension of this analysis may utlise EMI sources from the car, MCU and other components to analyse its robustness.

| Setting | Value | Reason |
|---|---|---|
| Frequency sweep | 0.5–3.0 GHz | covers L5 (1.176 GHz) and L1 (1.575 GHz) plus the inductor SRF at 2.8 GHz |
| Critical bands | 1164–1188 MHz and 1559–1606 MHz | per the antenna and receiver specs |
| Ports | 50 Ω at SMA pad and at `RF_IN` pad; both directions | need S11, S21, S22 |
| Mesh | at least 3 cells across the 0.32 mm trace and 3 cells through the 0.2104 mm dielectric | resolve the field under the trace |
| Materials | stackup from the PCB: prepreg εr 4.4, core εr 4.6, tan δ 0.02, copper 35 µm outer layers | JLCPCB's published values |
| Lumped parts | ideal first, then vendor S-parameter models | isolate layout effects from part effects |

### Steps

**Step 0 — Set up and prerequisites.** Install openEMS and KiCad-RFsim. 

**Step 1 — Validate the toolchain on the bare line.** Simulate a straight 0.32 mm line of the ANT3 length
(J9 to J10, no components).
- Check: the extracted Z0 against the hand calculation (54 Ω) and KiCad's calculator.
- Check: S21 (insertion loss) is small and S11 is low. Any large error here means the setup is wrong, not
  the board.
- Deliverable: a validated setup, plus the true line impedance including the coplanar pour (tests H5).

**Step 2 — Connector launch.** Add the SMA edge-launch footprint (`RFPC-SMA27-F`) to the bare line
(tests H6). Record S11 of the launch alone.

**Step 3 — Bias-tee and protection, one part at a time.** Starting from the bare line, add in this order,
recording S11 after each:
1. TVS pad and ground via (`D4`).
2. Bias-inductor tap: pads and stub with the ideal 56 nH, then with the Murata S-parameter model (H1, H2).
3. Series DC-block pads (`C27`) (H3).

**Step 4 — Pi filter.** Add the series `R35` pads and the shunt pads `C28`, `C29`, first empty, then with
0 Ω, then with simulated, realistic values (H4).

**Step 5 — Full chain.** Simulate connector to `RF_IN` pad with everything present (H7). Also record the
isolation from the RF line to the bias feed pin (H8). Repeat for chain 2 (ANT2) to see whether the two
layouts agree.

**Step 6 — Correlate with the real board.** When boards arrive:
- Measure the ANT3 coupon on a VNA and compare with Step 1 (this validates the simulation itself).
- Measure the populated chains at the SMA and compare with Step 5.
- If simulation and measurement disagree, fix the model before trusting any layout change.

**Step 7 — Decide.** For each hypothesis that fails its criterion, propose the smallest layout change
(trace width, pad shape, moving a part, removing a stub, ground-plane cutout), re-simulate that section
only, and record the result.

## 7. Acceptance criteria 

| Metric | Target | Bands |
|---|---|---|
| S11 looking into the SMA | −10 dB or better (−15 dB preferred) | 1164–1188 MHz and 1559–1606 MHz |
| S11 looking back from `RF_IN` | −10 dB or better | same |
| Insertion loss (SMA to `RF_IN`) | 0.5 dB or better | same |
| In-band ripple in S21 | under 0.3 dB | same |
| No resonance or notch in S21 | none between 1.0 and 2.0 GHz | wider check |
| RF-to-bias isolation (H8) | −30 dB or better | same |
| Line impedance | 50 Ω ±10 % (ideally ±5%)| full run of the line |

## 8. Risks and open questions

- The stackup now uses JLCPCB's published εr, but tan δ (0.02) is a generic assumption, and εr varies with frequency and resin content. Real values can still move the impedance by a couple of ohms.
- KiCad-RFsim is new and has 32 commits by one author, so Step 1 exists to catch its problems.
- Simulating lumped parts as ideal hides their parasitics; the vendor S-parameter step matters.
- Coarse meshes under-read impedance; a finer mesh raises run time.
- The simulation cannot model the 5 m antenna cable or the antenna's LNA output impedance. It tells us
  about the board only.
- ~~Open: are `C28`/`C29`/`C49`/`C50` meant to be populated~~ — resolved 2026-09-22: intentionally empty
  (a tuning allowance, not meant to be populated for this board rev). Step 4 and H4 correctly model
  them as open.
- Open: is ANT3 a calibration coupon? (unresolved)

## 9. Results log

| # | Hypothesis | Result | Notes | Run |
|---|---|---|---|---|
| H1 | L2 tap stub adds enough shunt C for RL under 15 dB | Pass (circuit model only) | ANT1/ANT2 worst-case S11 −15.1 to −19.3 dB across L5/L1, ideal L2 model. Not confirmed by a 3D run — Task E blocked (see below) | `RF Analysis/sim/chain_model.py` → `results/chain_model/*.s2p` |
| H2 | TVS pad/via add more C than datasheet 0.05 pF | Open | Not measured directly. Circuit model assumes the datasheet 0.05 pF (no vendor S-parameter file found) and passes; sensitivity sweep shows 10x that value would still pass, so this hypothesis has wide margin even if the true value is somewhat higher. No independent measurement of the real value | `RF Analysis/sim/chain_model.py` |
| H3 | Pad-width steps at 0402 DC-block/0Ω give a measurable Z bump | Pass (circuit model only) | Modeled as a generic 0.2 pF pad capacitance per part; sensitivity sweep shows S11 only fails above ~2.5x this (0.5 pF) and insertion loss above ~3x (0.6 pF) — comfortable margin. Not confirmed by 3D | `RF Analysis/sim/chain_model.py` |
| H4 | Empty pi-filter pads (C28/C29/C49/C50) add stub capacitance | Pass (circuit model only) | Modeled as open (no pad capacitance term added for unpopulated pads) — by construction this can't fail in the current model; the real board's fully-empty pad probably behaves close to this. Not confirmed by 3D | `RF Analysis/sim/chain_model.py` |
| H5 | 54 Ω trace (vs 50 Ω target) plus the coplanar pour leaves line Z off target | **Fail** | FD quasi-static solve (trustworthy per convergence check) gives Z0 = 54–56 Ω on ANT1/ANT2/ANT3, i.e. 8–12% high — inside the plan's ±10% band but outside the preferred ±5%. Closed-form (Hammerstad-Jensen) formula was checked and found invalid for this thin substrate; the FD solve is the number to trust. Smallest fix: widen the trace to about 0.40–0.42 mm (or use a thinner prepreg) to bring Z0 to 50 Ω | `RF Analysis/sim/task_a_line_impedance.py` |
| H6 | J7/J8 SMA launch is a bigger discontinuity than the filter parts | Open | Never reached — Task E (the run that would isolate the connector launch) is blocked by the port-configuration failures below | — |
| H7 | Whole chain has an in-band notch/ripple from interacting discontinuities | Pass (circuit model only) | Worst-case in-band ripple 0.008 dB (ANT1 L1), far under the 0.3 dB criterion, with ideal parts and approximate (straight-line) segment lengths. This needs 3D confirmation before it's trustworthy, which Task E could not deliver | `RF Analysis/sim/chain_model.py` |
| H8 | Bias-network RF leakage into VDD_RF through the tap | Open | Requires the full 4-port 3D run (port 4 = bias side) — never run. The circuit model doesn't build a 4th port, so it can't speak to this at all | — |

**Why H6–H8 are open:** Task E (the confirming 3D run) could not be completed on this stack-up. Three
port configurations were tried and each failed a different way: all-lumped ports give near-total
broadband reflection beyond ~5–6 mm port separation (numerically stable, evidence points to a
parasitic cavity/common-mode resonance in the ground pour — see `STATUS.md` "Task E prep"); microstrip
(msl) ports on a short bare-line test give a hard power-conservation violation; msl ports on the real
ANT1 chain (ports 1/2) diverge exponentially, continuing to grow well after the excitation pulse ends
(see `STATUS.md` "Task E: msl/cpw ports on the real ANT1 chain also fail" and
`results/laptop/ant1_4port_fine/run.txt`). No root cause was confirmed for any of the three failures
within the time spent; H6–H8 remain untested by simulation. The real-board VNA measurement in Step 6
is the next opportunity to get data on these, once boards arrive.
