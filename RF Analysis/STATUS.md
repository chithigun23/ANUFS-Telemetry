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

## Task C: a validation test that can detect a Z0 error (2026-09-22)

**Step 1: 7 mm fixed-port section with the Task B mesh fix, both ports.** `port_coverage.py` passed
(both ports on plane copper). Result: **the Task B trace-width fix did not fix the near-total-reflection
problem** first seen in `laptop/step1_long_fixed_fine_lumped`. S11 -0.9..-0.0 dB, S21 -35.9..-14.8 dB,
reciprocal, no power warning -- essentially unchanged from before the mesh fix. So that reflection is
not a mesh-resolution artifact; its real cause is still open, separate from the coarse/medium
divergence Task B addressed.

**Step 2: deliberately-wrong 0.60 mm trace.** Added `--trace-width` to `run_variant.py` (rescales the
actual F.Cu trace polygon about the line's own x, not just a label; leaves the surrounding pour where
it is). On the 7 mm baseline (already saturated near total reflection) it made barely any difference:
S11 -0.9..-0.0 -> -0.2..-0.0 dB. **The 7 mm test cannot detect a Z0 error, as the brief anticipated it
might not.** Re-ran on the known-good 5 mm baseline instead (both ports, which needed a new
`--excite-all` flag -- the saved model.json had `excite: [1]` from its original single-port run):
S11 went from -38.0..-23.1 dB (0.32 mm, correct) to -0.2..-0.0 dB (0.60 mm, wrong) -- an enormous,
unambiguous change. **The technique clearly works on a well-behaved baseline.** Note: 0.60 mm may put
the trace edge close enough to the ground pour to be an extreme case rather than a moderate Z0 shift;
a smaller perturbation (e.g. 0.40 mm) would show a more proportionate change if wanted later.

**Step 3: eps_eff from S21 phase vs Task A.** New `sim/experiments/eps_eff_from_phase.py`. On the 5 mm
baseline: eps_eff = 3.79 (L1/L5), against Task A's ANT3 FD-solve figure of 3.17 -- about 20% apart,
outside the 5% bar. Z0 from S11 (50*(1+S11)/(1-S11)) agrees better: 48.6 ohm against Task A's 53.9 ohm,
about 10% apart, still outside 5%. Likely reasons, not yet confirmed: Task A's 2D solve only modelled
the top dielectric layer down to In1.Cu, ignoring the rest of the real stack (In2.Cu, B.Cu); and a
lumped port has no explicit de-embedded reference plane the way an msl port does, so the phase-based
extraction's assumed 5.0 mm port-to-port length may not be the true electrical length. **Cross-check is
in the right ballpark but does not meet the stated 5% agreement bar; flagged, not resolved.**

## Task D: circuit model of the ANT1 and ANT2 chains (2026-09-22)

**Vendor Touchstone files: not obtained.** Murata SimSurfing's search UI (a legacy jQWidgets app) did
not return results despite a genuine attempt: found and cleared a duplicate-text bug in its search
input via its own jQuery API, triggered the real search button directly, checked the network requests
and DOM for a client-side dataset -- the results grid stayed empty throughout. User chose to proceed
with ideal component models rather than keep debugging it. L2/L3 keep the existing embed_l2.py model
(56 nH + 2.8 GHz self-resonant cap + an assumed 1.5 ohm); the 100 pF caps are ideal C0G; D4/D5 stay the
plan's assumed 0.05 pF (no public S-parameter file exists for the TVS regardless of SimSurfing).

**Topology and lengths: derived from the real board, not assumed.** `sim/chain_model.py` extracts
every component's actual pad position from `telemetry.kicad_pcb` (a rotation-aware S-expression
parser) and uses straight-line pad-to-pad distances as each segment's length -- an approximation (the
real routed trace has corners; understates true length most for the ~18.7 mm run into the receiver
pin). Which pad of each 2-pin part faces the RF line vs. ground was inferred from proximity; ANT1 and
ANT2 gave the same pattern independently, which is some cross-check on the inference. Full topology
and the derivation are documented in the script's own docstring.

**Result: both chains pass comfortably with the ideal-component model.**

| Chain | Band | S11 worst | Insertion loss worst | Ripple |
|---|---|---|---|---|
| ANT1 | L5 | -19.3 dB | 0.061 dB | 0.001 dB |
| ANT1 | L1 | -15.3 dB | 0.134 dB | 0.008 dB |
| ANT2 | L5 | -19.2 dB | 0.063 dB | 0.001 dB |
| ANT2 | L1 | -15.1 dB | 0.139 dB | 0.008 dB |

All well inside the plan's -10 dB / 0.5 dB / 0.3 dB criteria. Notably, this model uses Task A's
elevated Z0 (54-56 ohm, not 50) for the line sections, and S11 is still comfortably under -10 dB --
the chains are electrically short enough that the Z0 mismatch found in Task A doesn't threaten this
criterion by itself.

**Sensitivity sweep (0.5x-3x each parasitic):** pad capacitance (0.2 pF assumed) is the one that
matters -- S11 fails around 2.5x nominal (0.5 pF) and insertion loss around 3x (0.6 pF). Ground-via
inductance (0.5 nH), the TVS capacitance (0.05 pF), L2's assumed series resistance (1.5 ohm), and the
bias decoupling caps all have wide margin, no failure anywhere in 0.5x-3x.

**H1-H4/H7 (this model can speak to): pass comfortably, but still need 3D confirmation (Task E)** --
this is a circuit model with assumed/ideal parts and approximate lengths, not a measurement. H6
(connector launch) and H8 (RF-to-bias isolation) need the SMA and port-4 models this script doesn't
build; left for Task E. Results: `results/chain_model/ant1_chain.s2p`, `ant2_chain.s2p`.

## Task list

| Task | Status |
|---|---|
| 1. Clean up existing results | Done 2026-09-22 |
| A. Valid line impedance, no FDTD | Done 2026-09-22 — Z0 too high (54-56 ohm), needs a wider trace or thinner dielectric; user to decide |
| B. Fix the FDTD mesh before any new board run | Done 2026-09-22 — lumped-port trace-width fix built and verified (mesh only, not yet solved); z/open-air ratio jump identified but not fixed |
| C. A validation test that can detect a Z0 error | Done 2026-09-22 — works on the 5mm baseline, not on the still-broken 7mm one; eps_eff/Z0 cross-check with Task A misses the 5% bar (~10-20% off), flagged |
| D. Circuit model of each chain | Done 2026-09-22 — both chains pass comfortably (ideal components, vendor files not obtained); pad capacitance is the sensitive parasitic |
| D. Circuit model of each chain | Not started |
| E. One confirming 3D run | Not started |
