# Step 1 Variant — Short-Section Test

This document supplements `RF-Analysis-Plan.md` (Step 1). It does not replace it. It records why Step 1 is
being run on a short section of the ANT3 line instead of the full J9-to-J10 line, and how.

Status: **first run complete (2026-09-22): line impedance usable, S-parameters not valid.** See section 6.

## 1. Why the full line was not simulated

The first attempt simulated the full ANT3 line, from the `J9` pad to the `J10` pad (about 11 mm).

| Item | Full line | Short section |
|---|---|---|
| Mesh cells | about 3.0 million | about 0.43 million (118 x 94 x 39) |
| Timestep | about 1.2e-14 s | same |
| Steps needed | about 300,000 | about 300,000 (cap) |
| Solver speed (measured) | about 32 million cells/s on the Ryzen 3 3100 | same |
| Estimated time per port | 5-7 hours | about 40-60 minutes |

Cost is cells times timesteps. The timestep is set by the smallest mesh cell, and the smallest cell is set by
the 0.32 mm trace: the plugin puts 4 cells across a microstrip and snaps geometry to 0.02 mm. That is inherent
to a narrow trace on this stackup, not a fault in the plugin or the machine. The first full-line run reached
3,607 of about 300,000 steps in 7 minutes and was stopped.

## 2. What Step 1 needs to show

Step 1 has two goals (see the plan):

1. Confirm the simulation setup gives sensible numbers on a structure whose answer we already know.
2. Get the true line impedance of the 0.32 mm trace, including the effect of the coplanar ground pour.

A bare line is uniform, so both goals can be met with a section of it. The plugin's microstrip ports extract the
line impedance and propagation constant from the fields at each port, so they do not need the full 11 mm. A 5 mm
section is about 1/20 of a wavelength at L1. It should be enough for both goals, but it limits how accurately
the loss can be measured. The check is that Z0 read at port 1 and at port 2 agree; if they do not, the section
is too short and the test is rerun with a longer one.

## 3. What is simulated

| Setting | Value |
|---|---|
| Trace | 0.32 mm on F.Cu, straight, x = 18.26 mm, from y = 34.5 mm to y = 29.5 mm (5 mm) |
| Ports | microstrip ports at test pads P1 (y = 34.5) and P2 (y = 29.5), each 0.32 x 0.32 mm |
| Copper and vias | the real ground pours (F.Cu, In1.Cu, B.Cu) and the real ground vias, cropped to the box |
| Stackup | JLCPCB JLC04161H-7628 as set in the KiCad file (prepreg er 4.4, core er 4.6, loss tangent 0.02) |
| Crop box | x 14.0-22.5 mm, y 27.0-37.0 mm, plus 2 mm margin (inner air band) and 2 mm PML |
| Frequency sweep | 0.5-3.0 GHz, 251 points |
| Mesh preset | coarse (plugin preset; 4 cells across the strip) |
| Components | none (bare line, no lumped parts) |

The SMA connectors (`J9`, `J10`) are removed from the copy. The reference planes are at the test pads, so the
connector launch is not part of this step (that is Step 2).

The real `telemetry.kicad_pcb` is only read. The script edits a copy in `results/`.

## 4. What this test can and cannot tell us

Can tell us:
- Whether the toolchain reads the board and stackup correctly (plausible S11 and S21 for a uniform line).
- The line impedance of the 0.32 mm trace including the coplanar pour, compared with the hand estimate of
  about 54 Ω (plan, section 2.2).
- The insertion loss per millimetre of this trace on this stackup.

Cannot tell us:
- Anything about the connector launch, the bias tee, the TVS, the DC block or the pi filter (Steps 2 to 5).
- Loss over the full 11 mm, except by scaling the per-millimetre figure.
- Absolute accuracy at the coarse preset. The plugin's own tests show the coarse preset reads about 2 Ω low on
  a 50 Ω line, so a medium-mesh rerun is planned if the coarse result is within a few ohms of the estimate.

Sources of difference from the real ANT3 line:
- The ground pour's clearance to the trace was computed for the original full-length trace. It is kept as is.
- The test pads are the same width as the trace, so there is no pad step (the real SMA pads are 2.3 mm).
- The trace runs at x = 18.26 mm, as on the board (0.03 mm off the connector pad centre).

## 5. How to run

With KiCad's Python (needs `pcbnew`):

    "C:\Program Files\KiCad\10.0\bin\python.exe" "RF Analysis\sim\step1_ant3.py" coarse short

Set `DRY=1` to build the model and stop before the solver starts. Use `medium` or `fine` in place of `coarse`
for a finer mesh, at much higher cost (about 8x cells and 2x steps per step up in preset; check the estimate
first). Set `RF_TAG` (for example `desktop` or `laptop`) so results from different machines stay separate.
Output goes to `RF Analysis/results/<RF_TAG>/step1_<mode>_<mesh>/` (the first 5 mm run, before machine folders
existed, is in `RF Analysis/results/step1_short_coarse/`):

| File | Content |
|---|---|
| `ant3_cropped.kicad_pcb` | the cropped copy of the board that was simulated |
| `model.json` | the geometry sent to the solver |
| `solver.log` | full openEMS log |
| `results.s2p` | S-parameters (Touchstone) |
| `summary.csv` | worst-case S11 and S21 in the L5 and L1 bands |

## 6. Results

| Date | Mesh | Line Z0 (ohm) | S11 worst L5 / L1 (dB) | S21 worst L5 / L1 (dB) | Notes |
|---|---|---|---|---|---|
| 2026-09-22 | coarse | 46.8 (port 1), 46.1 (port 2), at 1.75 GHz; eps_eff 3.30 / 3.19 | not valid | not valid | see below |
| 2026-09-22 | coarse, 9.5 mm | not valid (solver diverged) | not valid | not valid | desktop and laptop identical, see section 7 |

**Run details (coarse, 118 x 94 x 39 cells):** timestep 1.77e-14 s. Port 1 stopped at about 118,000 steps and
port 2 at about 110,000 steps, each when the energy fell below the -40 dB criterion, after about 24 minutes each.
Solver speed 33-38 million cells/s on the Ryzen 3 3100.

**Line impedance (usable):** the plugin reads 46.8 ohm at port 1 and 46.1 ohm at port 2, at 1.75 GHz, with an
effective dielectric constant of 3.30 and 3.19. The two ports agree within 0.7 ohm. This is lower than the hand
estimate of about 54 ohm (`RF-Analysis-Plan.md`, section 2.2), which ignored the ground pour beside the trace, as
expected. Coarse meshes under-read impedance by up to about 2 ohm in the plugin's own tests, so the true value is
probably 46-49 ohm. Not yet confirmed at a finer mesh.

**S-parameters (not valid):** the solver's own check reports that port 1 and port 2 give out more power than they
take in (sum of |S|^2 up to 116,536 and 2,977; it must be at most 1). The written S11 is +35 to +51 dB and S21 is
about +8.5 dB, which is impossible for a passive line. The far-field figure for port 1 also came out as NaN. These
numbers must not be used or quoted.

**Likely causes, not yet tested:**
1. The 5 mm section is too short: the plugin's line ports de-embed over a length of line, and two ports 5 mm apart
   may have overlapping measurement regions.
2. The coarse mesh is too coarse across the 0.32 mm strip (the solver's warning points at this).
3. Test pads the same width as the trace (0.32 mm) may not be handled well as microstrip ports.

**Next runs to separate these causes:** (a) the same section at 10 mm, coarse mesh (about 2 hours for both ports);
(b) the 5 mm section at the medium mesh. A run of the plugin's own validation with a narrow 0.32 mm trace would
also show whether the plugin handles this trace width.

## 7. 9.5 mm run (desktop and laptop): the solver diverges

Both machines ran mode `long` (9.5 mm section, coarse mesh, 134 x 110 x 39 = 574,860 cells, timestep 1.666e-14 s).
The results are identical on both, to the second decimal (`summary.csv`: S11 -11.12 / -11.13 dB, S21 +18.9 dB;
not physical), which shows the failure is deterministic and not a fault of one machine. The desktop took about an
hour per excitation, the laptop (Core Ultra 7 258V) about 1.6 times faster.

**What the log shows:** the field energy is bounded until about 60,000-75,000 steps, then grows exponentially:
about 1e-12 at 75,000 steps, 1e-5 at 100,000, 20 at 125,000, 1e8 at 150,000, and 7e27 at 225,000 steps, reaching
infinity at about 230,000 steps. The line impedance read at port 1 is meaningless (50 - 9.8j ohm with an
effective dielectric constant of 106), and the far field is NaN. This is a late-time numerical instability, not a
geometry error. Port results from a diverging run cannot be used.

**Reading of the earlier 5 mm result:** in that run the energy was still decaying (down to -35 dB at 110,000
steps) when the -40 dB stopping criterion ended the run. An instability that starts at 60,000-75,000 steps and
grows for tens of thousands of steps could have contaminated the port data before then without being visible in
the energy. The invalid 5 mm S-parameters are therefore probably the same fault, but this is not proven.

**Diagnostics under way (2026-09-22):**
| Machine | Change | Question |
|---|---|---|
| laptop | timestep factor 0.5, step cap 600,000, port 1 only | is the instability a timestep (Courant) problem? |
| desktop | copper as a perfect conductor (`sim/experiments/runner_pec.py`), port 1 only | does the finite-conductivity copper sheet model cause it? |
