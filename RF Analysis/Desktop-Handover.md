# Handover for the desktop session (written by the laptop session, 2026-09-22)

This file is for the Claude session on the desktop. The user asked for it. Read it fully before running anything. It
tells you which results to trust, which of the current models and scripts are wrong, what to run next, and how to
store results in git. The rules in `Laptop-Handover.md` section 2 still apply (do not modify the PCB, the schematic
or `RF-Analysis-Plan.md`; report faithfully; do not tune settings until a result "looks right").
Where this file disagrees with `Laptop-Handover.md` sections 3 and 4 (Stage A and Stage B as written there),
this file is newer.

Everything under "verified" below was checked by the laptop session against the files in `RF Analysis/results/`.
Anything marked "not verified" is a claim or a guess, and is labelled as such.

## 1. What to trust

**Valid board data (one geometry, reproduced on both machines).** 5 mm bare ANT3 line, fine mesh (1.165 mm),
lumped ports: `results/desktop/step1_short_fine_lumped/` and the laptop's long rerun of the same model,
`results/laptop/step1_short_fine_lumped_long/`. S11 -28.8 dB (L5) and -26.4 dB (L1); S21 -0.04 and -0.05 dB;
max |S11|^2+|S21|^2 = 0.992 (passive). The long rerun (early stop off, 7.9 ns) agrees with the desktop run to
0.0003 in |S11| and 0.0014 in |S21|, and its energy decays monotonically to -85 dB, so this result is stable.
Only port 1 was excited, so reciprocity is unchecked. It is not a Z0 measurement (lumped ports do not extract one).

**Toolchain validation (evidence the tools work, not board data).** All stable, passive, sensible:
`laptop/validation_msl_1_6`, `laptop/validation_msl_0p5_6`, `desktop/validation_msl_medium_0p5_3`,
`desktop/validation_stripline_long`. Z0 on the microstrip validation board reads 47.4-47.8 ohm.

**Stable but NOT usable as results (the model is wrong, see section 2).**
- `desktop/step1_long_fine_lumped` (9.5 mm, fine). Passive but opaque (S21 -65 to -60 dB in the bands).
- `desktop/ant1_4port_fine` (first ANT1 4-port run). Stable, passive (1.003), reciprocal (0.005), but it modelled
  C28/C29 as 0.25 nH shunts and left D4 open, so S21 -37 dB is not a real chain result.

**Invalid (diverged, or the plugin warned that a port gives out more power than it takes in).** Every coarse-mesh
and medium-mesh run of a board model, on both machines, including the laptop runs with timestep factor 0.5,
lossless dielectric, F.Cu signal-only, lumped ports and the medium mesh (`laptop/step1_short_medium_lumped`,
2.33 mm cells, inf at 106,045 steps). Also `validation_msl_1_3`, `validation_msl_fours_*` (validation board at
a coarse mesh of 4.7 mm or more).

## 2. Problems found in the current models and scripts

### 2.1 The 9.5 mm ("long") test section has its ports on the SMA clearance cutouts (verified)
In `results/desktop/step1_long_fine_lumped/model.json` both ports have no reference-plane copper under them:
port 1 at (18.26, -36.00) and port 2 at (18.26, -26.50), reference layer `In1.Cu` (also `In2.Cu` and `B.Cu`) are
empty at both points. Along x = 18.26 the reference plane exists only from about y = -35.75 to y = -27.5. A
lumped port needs copper on its reference layer to return current; without it the result is total reflection,
which is what the run shows (S11 -0.1 dB, S21 -65 dB). The earlier coarse microstrip runs on this section read
Z0 = 3.9+4.7j ohm and eps_eff = 1199 at port 2, which fits the same defect (port 2 has the longer bare stretch).
So the 9.5 mm section was not a fair test of a longer line, and the "does a longer line fix it" question in
`Laptop-Handover.md` section 3 was answered with a broken test.
The 5 mm section (ports at y = -34.5 and -29.5) and all four ANT1 ports are on copper.

How this was checked: `sim/experiments/port_coverage.py <model.json>` (new, standard library only). It prints
for every port whether the signal layer and the reference layer have copper there, plus a coverage strip along a
two-port line. Method caveat: it uses an even-odd point-in-polygon test on the model's polygons, which is right for
keyholed or separate-ring holes; if a surprising result appears, look at the board before trusting it.

Suggested fix (you own `step1_ant3.py`; this is a suggestion, not something the laptop changed): in `SECTIONS`,
`"long"` currently has end coordinates `(36.0, 26.5)`. Use ends that sit on plane copper, for example `(35.0, 28.0)`
(a 7 mm section, ports 0.75 mm and 0.5 mm inside the plane), and check the new model with `port_coverage.py`
before running it.

### 2.2 The fine mesh is stable on the 5 mm model (verified); it is not yet checked on the other fine models
`step1_short_fine_lumped` ended at 31,600 steps (about 2.1 ns) on the default -40 dB energy criterion, before the
point where the coarse and medium runs started growing (between about 1.3 and 2.5 ns). So that run alone could
not show whether the fine mesh is stable. The laptop reran the same model with the early stop off
(`--endcrit 1e-12`, cap 120,000 steps, 7.9 ns): `results/laptop/step1_short_fine_lumped_long/`. The energy decays
monotonically from the end of the excitation to -85.1 dB, with no growth in any of the 94 progress lines after
step 60,000, no power warning, and S-parameters that match the early-stopped run to 0.0003 (|S11|) and 0.0014
(|S21|). So for the 5 mm model the fine mesh is stable and the early-stopped result was not contaminated.

Not yet checked: the fine mesh on the ANT1 model (about 700,000 cells, different layout, more parts) and on the
9.5 mm section once its ports are fixed. For ANT1, run port 1 once with `--endcrit 1e-12` and a cap that covers
at least 6 ns of simulated time before trusting the default early stop there.

The desktop session reported that the coarse preset is the root cause. The laptop's medium result agrees that the
medium preset is not enough on our stack-up (26.2 e-folds per ns of energy growth, the same rate as the coarse
lumped runs), whereas the plugin's own validation board is stable at a 2.355 mm cell. So the trigger is a property
of our stack-up plus cell size, not the cell size alone, and why the coarse and medium presets diverge is still
unexplained. It only needs explaining if a fine-mesh run turns out not to be stable.

### 2.3 ANT1 (`step5_ant1_multiport.py`)
- Empty pads C28 and C29: dropped from the lumped list in the current script (the plugin turns a value of 0 into a
  0.25 nH shunt to ground). Good. The first `desktop/ant1_4port_fine` run predates this fix, so it is not usable.
- D4 (TVS) has no value in the model and is left open; its 0.05 pF is only added in the circuit stage
  (`embed_l2.py` does L2 only, not D4).
- The L2 model in `embed_l2.py` has an assumed 1.5 ohm series resistance and a 2.8 GHz self-resonance; replace it
  with the Murata S-parameters when available.
- The lumped ports at L2 have never been validated on this board.
- Stage B (this job) is on hold as far as the laptop is concerned until section 3 items 1-3 are closed. That is the
  laptop session's recommendation; the user has not decided it.

### 2.4 There is still no valid line impedance
The Z0 numbers we have (46.8 / 46.1 ohm from the 5 mm microstrip-port runs) come from runs whose S-parameters were
invalid, and they disagree with the 54 ohm hand calculation in the plan by about 7 ohm, which is more than the
"few ohms" `Short-Section-Test.md` allows for a coarse mesh. Lumped ports do not extract Z0. Deliverable 2 of
plan Step 1 (true line impedance including the coplanar pour, hypothesis H5) is open.

## 3. What to run, in order
1. Run `port_coverage.py` on every model.json before spending solver time on it. Do not run a model that fails it.
2. Fine-mesh stability: done for the 5 mm model (section 2.2). For the ANT1 model run port 1 once with
   `--endcrit 1e-12` and a cap of at least 6 ns, and if it stays stable the default early stop is acceptable for
   the other ports.
3. 9.5 mm section with the ports moved onto the plane (section 2.1), fine mesh, both ports excited so reciprocity
   can be checked.
4. ANT1 4-port at the fine mesh with the fixed script (you are already doing this). Excite all four ports.
5. A valid Z0: decide with the user how to get it (microstrip ports at the fine mesh, a 2D field solve, or
   from a valid longer lumped-port line), then compare with 54 ohm and KiCad's calculator.

Do not spend more time on coarse or medium runs of board models, or on more diagnostics of why the coarse mesh
diverges, unless the fine mesh turns out not to be stable.

Speeds seen (MCells/s): desktop 33-35 on the fine runs; laptop 37-60, higher on larger models. Fine runs of the
5 mm model take about 5-15 minutes, ANT1 about 9 minutes per port on the desktop.

## 4. Storing results in git

Only small files are committed. Large files never go in git.
- Commit: `results.s2p` / `results.s4p`, `summary.csv`, `lines.json`, `solver*.log`, and a small `run.txt` with
  the exact command that made the run.
- Never commit: `exc*/` folders, `farfield_*.json`, `*.kicad_pcb` (the cropped copies are about 2.8 MB), `*.h5`, or
  anything over about 1 MB. `results/.gitignore` already excludes the first three. Check `git status` and file
  sizes before every commit.
- `model.json`: it is 250-550 KB and about three quarters of everything the results folders add to the repository
  (about 5.8 MB of about 7.5 MB so far), and most copies are near-identical variants. Commit it only for the
  first run of each distinct geometry. For a variant, put the `run_variant.py` command in `run.txt` instead.
  Add files by name, not with `git add <folder>`.
- Solver logs used to be ignored by the root `.gitignore` (`*.log`); `results/.gitignore` now un-ignores
  `solver*.log`, so `git add -f` is no longer needed.
- One folder per run, named for what changed, under `results/<RF_TAG>/` (`RF_TAG=desktop` here).
- Commit with `git commit -F <message file>`; a message with double quotes inside `-m` breaks PowerShell's
  argument passing to git and the commit silently does not happen. Do not add a Co-Authored-By trailer
  (the user does not want one). Always `git pull --rebase --autostash` before `git push`.

## 5. Tools

| Tool | Use |
|---|---|
| `sim/experiments/port_coverage.py` | check every port has reference copper under it |
| `sim/experiments/run_variant.py` | flags: `--tand0 --endcrit --maxsteps --fstart --fstop --mesh --bc --pec --solid-planes --signal-only` |
| `sim/bands.py` | band S11/S21 and the passivity check for a two-port `.s2p` |
| `sim/embed_l2.py` | put L2 back on an ANT1 `.s4p` |
| `sim/experiments/repostprocess.py` | recompute S-parameters offline from a run |

## 6. What to report for every run
Cells, timestep, steps done, wall-clock time, whether the energy criterion or divergence ended the run,
max sum |S|^2 per excited column, reciprocity if both ports were excited, the printed line impedance if the ports
are microstrip, and any plugin warnings. Say plainly when a run is invalid.
