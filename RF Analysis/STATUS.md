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

## Task B: fix the FDTD mesh before any new board run (2026-09-22)

`sim/experiments/mesh_check.py` reuses the plugin's own mesh-building code (`runner._port_geometry`,
`runner._mesh`, `SmoothMeshLines`) so it reports the exact mesh a real run would use.

**Item 2 hypothesis (neighbour-cell ratio above ~2 causes the divergence): partly confirmed, not clean-cut.**
Compared the valid fine mesh (`_keep/desktop/step1_short_fine_lumped`) against the same 5 mm model
regenerated at the coarse preset (which always diverged when actually run):

| Axis | Fine (stable) worst ratio | Coarse (diverges) worst ratio |
|---|---|---|
| x | 4.4x | 15.0x |
| y | 9.6x | 20.3x |
| z | 4.6x | **36.0x** |

A flat "> 2" threshold doesn't separate them (the stable fine mesh already reaches 9.6x), but coarse is
consistently 2-8x worse than fine in every axis, worst in z. Both worst-z jumps sit at the same
z = 1.591 mm, just above the top copper layer, where a tiny anchor-forced cell (0.053 mm, from a
via/port snap) sits directly next to the open-air cell sized by the mesh preset (0.24 mm fine, 1.89 mm
coarse). That specific transition is the leading suspect; not fixed in this pass (see below).

**Separate, independent defect found: lumped ports get no trace-width mesh refinement at all.**
`runner.py`'s `_mesh()` only grades the mesh across the strip for `"msl"`/`"cpw"`/`"stripline"` ports
(see its comment: "before, only the CPW branch existed"). Every run in this project uses `"lumped"`
ports, which fall through with none of that grading — measured with `mesh_check.py`: only **2 cells**
across the 0.32 mm trace, at both the coarse and the fine preset.

`sim/experiments/runner_meshfix.py` (a monkey-patched copy, plugin itself untouched) generalises that
same grading to lumped ports: raises the trace-width cell count from 2 to 4 on both the valid fine
model and the diverging coarse model, verified with `mesh_check.py`, without introducing new bad
ratios elsewhere. Getting this right took two failed attempts, documented in the file: a naive
insertion first created a new 26x ratio pathology (a candidate line landed 6 um from an unrelated
anchor); the fix merges a candidate into a nearby existing line instead of rejecting or duplicating it,
sized relative to the target cell for that port, not to a fixed fraction of the FDTD resolution.
This variant does **not** touch the z/open-air ratio jump above; if a solve with it still diverges,
that jump is the next thing to fix.

**Item 4 (`Unused primitive (LinPoly)` in `cu_*`, `(Cylinder)` in `vias`): harmless in the cases checked.**
Both warnings, on all four copper layers and on vias, appear in the two currently-known-valid, physically
correct runs (`step1_short_fine_lumped` and its long rerun) — passive, low-loss S-parameters despite the
warnings. They appear alongside the plugin's own "copper ... extends beyond the simulation domain and is
cut at the boundary" warning in the same logs, which is the plugin's own explanation for exactly this.
Not re-checked on a geometry where a via sits away from the domain edge; treat as harmless for now, revisit
if it shows up on a model where that explanation doesn't apply.

**Item 5 (policy):** every run from now on excites all ports and checks reciprocity (|S_ij - S_ji| <= 0.02
per section 0's validity rule), not just port 1.

## Task list

| Task | Status |
|---|---|
| 1. Clean up existing results | Done 2026-09-22 |
| A. Valid line impedance, no FDTD | Done 2026-09-22 — Z0 too high (54-56 ohm), needs a wider trace or thinner dielectric; user to decide |
| B. Fix the FDTD mesh before any new board run | Done 2026-09-22 — lumped-port trace-width fix built and verified (mesh only, not yet solved); z/open-air ratio jump identified but not fixed |
| C. A validation test that can detect a Z0 error | Not started |
| D. Circuit model of each chain | Not started |
| E. One confirming 3D run | Not started |
