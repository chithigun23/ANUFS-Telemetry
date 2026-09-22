# RF analysis status

Kept-run tracker for `Next-Steps-Sonnet.md`, which is the authoritative task brief. Updated after every task in
that file. The old `Laptop-Handover.md`, `Desktop-Handover.md` and `Short-Section-Test.md` are retired to
`archive/` and superseded by this file and the brief.

Everything not listed below (all coarse- and medium-mesh board runs, the 9.5 mm runs with ports on the SMA
clearance cutouts, and the first ANT1 run) was removed on 2026-09-22 as invalid; see the "remove invalid RF runs"
commit for the full list and confirmation that each was in git history first.

## Kept runs (`results/_keep/<machine>/...`)

| Run | Verdict | Meaning |
|---|---|---|
| `desktop/step1_short_fine_lumped` | **Valid** | Only valid board result so far: 5 mm ANT3 bare line, fine mesh (1.165 mm), lumped ports. S11 -28.8 dB (L5) / -26.4 dB (L1), S21 -0.04 / -0.05 dB, passive (max sum\|S\|^2 0.992). Port 1 only, so reciprocity unchecked. Not a Z0 measurement. |
| `laptop/step1_short_fine_lumped_long` | **Valid** | Same model as above, early stop disabled, run to 7.9 ns. Confirms the fine mesh is stable on this model (energy decays to -85 dB, no growth) and that the early-stopped result above was not contaminated (agrees to 0.0003/0.0014). |
| `laptop/validation_msl_1_6` | **Valid (toolchain check)** | Plugin's own microstrip validation board, 1-6 GHz, early stop off, stable to -107.6 dB. Evidence the solver/plugin work correctly; not board data. |
| `laptop/validation_msl_0p5_6` | **Valid (toolchain check)** | Same validation board, 0.5-6 GHz. Stable. Shows a low start frequency alone does not trigger the instability seen on board models. |
| `desktop/validation_msl_medium_0p5_3` | **Valid (toolchain check)** | Validation board at the medium mesh, 0.5-3 GHz. Stable — the plugin itself is fine at this cell size; the instability on board models is specific to our stack-up. |
| `desktop/validation_stripline_long` | **Valid (toolchain check)** | Plugin's own 4-layer stripline validation, run to the full 300,000-step cap. Stable to -97 dB, confirming the solver build is fine on a multilayer stack-up. |
| `laptop/step1_long_fixed_fine_lumped` | **Valid, but flagged** | 7 mm ANT3 section, ports moved off the SMA clearance cutouts onto plane copper, fine mesh, lumped ports, both ports excited. Passes every check in section 0 (no divergence, no power warning, passive, reciprocal), but S11 is near 0 dB and S21 is -15 to -36 dB — near-total reflection. Matches the desktop's independent finding on its ANT1 rerun. Not yet explained; candidate input to Task C. |

## Not reviewed / left alone
`desktop/step1_long_fine_ports_on_plane` — only a `model.json` committed, no results yet. Left for the desktop
session; will be reviewed once it produces a result.

`desktop/step1_long_fine_msl` (2026-09-22, reviewed and removed) — same 7 mm fixed-port section, microstrip
ports, fine mesh. Port 2 gives out more power than it takes in (max sum\|S\|^2 = 1.50), S21 -24 to -26 dB.
Matches the laptop's earlier finding: microstrip ports fail on this geometry even at the fine mesh. Removed.

## Task A: line impedance, no FDTD (2026-09-22)

`sim/task_a_line_impedance.py` measures the actual coplanar gap from the board's cached Ground Pour fill
polygon (not a design-rule guess) and computes Z0 two ways.

| Line | Gap (median, mm) | Closed-form GCPW | FD Laplace solve (converged, ±2%) |
|---|---|---|---|
| ANT1 | 0.315 | 115.7 ohm, eps_eff 1.88 | **55.5 ohm**, eps_eff 3.23 |
| ANT2 | 0.355 | 122.7 ohm, eps_eff 1.83 | **55.7 ohm**, eps_eff 3.24 |
| ANT3 | 0.205 | 94.5 ohm, eps_eff 2.09 | **53.9 ohm**, eps_eff 3.17 |

Plain microstrip (no coplanar gap) for comparison: 57.3 ohm, eps_eff 3.27.

**The two methods disagree by 75-120%, and the closed-form is the one to distrust.** The substrate here
(h = 0.2104 mm) is thin compared with the gap (0.2-0.36 mm), so the field couples mostly straight down to the
In1.Cu plane rather than sideways to the coplanar strips — closer to microstrip than open CPW. As h shrinks,
eps_eff should rise toward the board's er (4.4); the closed-form GCPW formula (Ghione-Naldi/Wadell, sinh-scaled
finite-ground term) goes the other way here (eps_eff ~1.8-2.1, below even a standalone open-CPW estimate), so
it is being used outside its valid range. The FD solve moved less than 2% under a doubled domain and a finer
grid, so it is converged; its numbers also sit close to the plain-microstrip figure, which is the physically
expected limit for a thin substrate. **Take the FD figures as the current best estimate of Z0.**

**Result: Z0 is high, not low.** All three lines read 54-56 ohm, mostly from the plain trace/substrate
geometry, not primarily from the coplanar gap (which only lowers it about 2-3 ohm from the plain-microstrip
57.3 ohm). That is 8-11% above the 50 ohm target, outside the +/-5% band. Closing the gap further would only
buy 2-3 more ohm; hitting 50 ohm needs a wider trace or a thinner dielectric, not a smaller coplanar gap.
Widening the trace to about 0.40-0.42 mm (rough estimate from the microstrip trend, not yet solved exactly)
would be the smallest layout change to try. **This has not been changed on the board; it needs a decision.**

Caveat: 6-9 of ~35-43 sample points per line found no In1.Cu copper directly under the trace (component pads
or via clearances). The Z0 above assumes typical coverage; it is not the value exactly at those points.

## Task list

| Task | Status |
|---|---|
| 1. Clean up existing results | Done 2026-09-22 |
| A. Valid line impedance, no FDTD | Done 2026-09-22 — Z0 too high (54-56 ohm), needs a wider trace or thinner dielectric; user to decide |
| B. Fix the FDTD mesh before any new board run | Not started |
| C. A validation test that can detect a Z0 error | Not started |
| D. Circuit model of each chain | Not started |
| E. One confirming 3D run | Not started |
