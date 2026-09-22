# Next steps: task brief for the agent (supersedes both handovers)

Read this fully before running anything. It replaces `Laptop-Handover.md` and `Desktop-Handover.md`.
Background: `RF-Analysis-Plan.md`. Setup: `Setup-and-Software.md`.

## 0. Rules
- Do not modify `telemetry.kicad_pcb`, the schematic, or `RF-Analysis-Plan.md` (except its section 9 results log).
  Propose other plan changes to the user.
- Report faithfully. A run is **invalid** if it diverged, if the solver warned that a port "gives out more power than it
  takes in", if any column has a sum of |S|^2 above 1.01, or if reciprocity fails (|S21 - S12| > 0.02). Never tune
  settings until a result "looks right".
- Commit rules: small files only (`*.s2p/s4p`, `summary.csv`, `solver*.log`, `run.txt`); no `exc*/`, `*.h5`,
  `*.kicad_pcb` or far-field files; nothing over 1 MB. Commit with `git commit -F <file>`. **No Co-Authored-By
  trailer.** Run `git pull --rebase --autostash` before pushing. Commit or push only when the user says so.
- Ask before installing software that `Setup-and-Software.md` does not list.

## 1. Clean up the existing results (do this first)

Review verdict: the existing analysis produced **almost no usable board data**. Every coarse- and medium-mesh board
run diverged or broke passivity. The 9.5 mm "long" runs had their ports on the SMA clearance cutouts. The first ANT1
run modelled the empty pads as shorts. The Z0 figure of 46.8 ohm comes from invalid runs and must not be quoted again.

**Keep** (move into `results/_keep/`, keeping the machine subfolder):
| Folder | Why |
|---|---|
| `desktop/step1_short_fine_lumped` | only valid board result (5 mm line, passive, stable) |
| `laptop/step1_short_fine_lumped_long` | confirms the result above is stable |
| `laptop/validation_msl_1_6`, `laptop/validation_msl_0p5_6`, `desktop/validation_msl_medium_0p5_3`, `desktop/validation_stripline_long` | evidence the toolchain works on the plugin's own boards |

**Review before deciding:** `laptop/step1_long_fixed_fine` and `laptop/step1_long_fixed_fine_lumped` are new,
uncommitted and not yet reviewed. Apply the validity checks in section 0 (use `sim/bands.py`). If a run is valid, keep
it and log it. If not, delete it.

**Delete** every other folder under `results/`: all `*_coarse*` and `*_medium*` runs, `desktop/step1_long_*`,
`desktop/ant1_4port_fine`, `laptop/validation_msl_1_3`, `desktop/validation_msl_fours_*`, and the untagged
`results/step1_short_coarse/`. Before deleting:
1. List the exact folders and total size to the user and wait for a yes.
2. Confirm that each committed folder is in git history (`git log -- <path>`), so it can be recovered.
3. Use `git rm -r` for tracked folders and a plain delete for untracked ones. Commit as one "remove invalid RF runs"
   commit, after the user approves.

Then archive `Laptop-Handover.md`, `Desktop-Handover.md` and `Short-Section-Test.md` into `RF Analysis/archive/`,
and create `RF Analysis/STATUS.md`: one table of every kept run, marked valid or invalid, with its one-line meaning.
Keep STATUS.md up to date after every task below.

## 2. Task A: valid line impedance, no FDTD (closes H5)
1. From the PCB file (read only), measure the F.Cu pour-to-trace gap along the ANT1, ANT2 and ANT3 lines, and also
   check In1.Cu under the trace.
2. Compute Z0 as a grounded coplanar waveguide: 0.32 mm trace, measured gap, h = 0.2104 mm, εr 4.4, 35 µm copper.
   Use two independent methods: (a) closed-form GCPW formulas in Python (Wadell), and (b) a 2D cross-section field
   solve (an openEMS 2D-like quasi-static solve, or a simple finite-difference Laplace solver written in numpy).
   Also report the plain microstrip value (about 54 ohm) for comparison.
3. Report Z0 and εeff. If Z0 is outside 50 ohm ±5 %, give the width (and/or gap) that hits 50 ohm, and **tell the
   user**. Do not change the board.

## 3. Task B: fix the FDTD mesh before any new board run
1. Write `sim/experiments/mesh_check.py`. For a `model.json`, or for the mesh the runner builds, it prints the
   min/max cell in x, y and z, the worst ratio between neighbouring cells, the number of cells across the trace
   width, and the number of cells through the prepreg.
2. Run it on the kept valid run and on one of the deleted diverged coarse runs (regenerate it with `DRY=1`).
   Hypothesis to test: the divergence is caused by a neighbouring-cell ratio above about 2, where 0.02 mm snapping
   sits next to large cells. Report the result either way.
3. Add mesh smoothing (`CSXCAD.SmoothMeshLines`, ratio ≤ 1.5) and local refinement (≥ 3 cells across the trace,
   ≥ 3 through the prepreg) as a runner variant in `sim/experiments/`. Do not edit the plugin in place.
4. Find out what the `Warning: Unused primitive (LinPoly) in cu_*` and `(Cylinder) in vias` lines in the solver logs
   refer to: copper cut off outside the domain (harmless) or copper missing from the structure (a bug). Report which.
5. From now on, excite **all** ports in every run and check reciprocity.

## 4. Task C: a validation test that can detect a Z0 error
1. Run the 7 mm `long_fixed` section with the Task B mesh, check `port_coverage.py` first, and excite both ports.
2. Run the same section with a deliberately wrong 0.60 mm trace. S11 must differ clearly from the 0.32 mm run.
   If it doesn't, the test can't detect Z0 errors and you should say so.
3. Extract εeff from the phase of S21 and compare it with Task A. Agreement within 5 % validates the setup.

## 5. Task D: circuit model of each chain (tests H1–H4, H7)
1. Download the vendor Touchstone files: Murata LQW15AN56NG00D (SimSurfing), a 100 pF C0G 0402 (Murata GRM155
   series), and the Littelfuse PESD0402-140 TVS. Tell the user each filename and source before downloading. Store
   them in `RF Analysis/models/`.
2. In scikit-rf (KiCad's Python), write `sim/chain_model.py`: line sections at the Task A Z0 and εeff, with lengths
   from the PCB, pad shunt capacitances (starting at 0.2 pF per 0402 pad, 0.3 pF for the TVS pad), ground-via
   inductance (about 0.5 nH), the L2 bias tee from the vendor file terminated by C30/C31/R39, D4, C27, R35, and the
   empty C28/C29 pads as capacitance only.
3. Replace the assumed values in `embed_l2.py` (1.5 ohm, 2.8 GHz SRF) with the Murata file.
4. Output: S11 and S21 over 0.5–3 GHz, and a sensitivity sweep of each parasitic (0.5× to 3×). For each one, report
   the value at which a criterion in plan section 7 fails. Mark which hypotheses pass comfortably and which need 3D
   confirmation.
5. Do ANT1 and ANT2 separately, using each chain's own lengths.

## 6. Task E: one confirming 3D run (H6–H8), only after B and C pass
1. ANT1 4-port with the Task B mesh. Put D4 into the model as 0.05 pF, not left open. Excite all four ports.
   Stability check: port 1 with `--endcrit 1e-12` and at least 6 ns before trusting the early stop.
2. Embed L2 using the vendor file, then compare with Task D. Where they differ by more than 3 dB in S11, the circuit
   model's parasitic estimate is wrong: update it.
3. Run the J7 SMA launch as its own run (Step 2, H6).
4. Report the RF-to-bias isolation at port 4 (H8).

## 7. Finish
- Fill in `RF-Analysis-Plan.md` section 9 with one row per hypothesis H1–H8: pass, fail or open, the number, and the
  run folder.
- Ask the user whether C28/C29/C49/C50 are meant to be populated. It changes Step 4.
- Give the user a short summary: which acceptance criteria pass, which fail, and the smallest suggested layout change
  for each failure.

## Per-run report (every run)
Cells, min cell, worst neighbouring-cell ratio, timestep, steps done, wall-clock time, how the run ended (energy
criterion, step cap or divergence), max sum |S|^2 per column, reciprocity, and any plugin warnings. Say plainly when
a run is invalid.
