# Handover: RF simulation jobs for the laptop

This file is written for a Claude Code instance (or a person) picking up work on a second machine. Read it fully
before running anything. Background is in `RF-Analysis-Plan.md`; software setup is in `Setup-and-Software.md`;
the results of the first desktop run are in `Short-Section-Test.md`.

## 1. Context in six lines

- Project: ANUFS-Telemetry, a KiCad 10 PCB with two GNSS receivers (LC29H) fed by active antennas.
- Concern: the bias-tee, TVS, DC-block capacitor and pi filter on each antenna line might cause reflections that
  degrade the signal at the receiver's `RF_IN` pin (L1 1559-1606 MHz, L5 1164-1188 MHz).
- Method: 3D full-wave simulation (openEMS, FDTD) of the real PCB layout, using the RFsim plugin's board reader and
  solver runner.
- Status on the desktop (2026-09-22): a 5 mm bare-line test gave a usable line impedance (about 46-47 ohm) but
  **invalid S-parameters**: the solver reported ports giving out more power than they take in. The cause is not yet
  known. Do not trust any S-parameter result from this toolchain until that is understood.
- This machine therefore runs **Stage A first** (a diagnostic, section 3). **Stage B** (section 4) only starts after
  the user has reviewed Stage A.
- The desktop is free to run other jobs in parallel.

## 2. Rules for this task

- Do not modify `telemetry.kicad_pcb`, the schematic, or `RF-Analysis-Plan.md`. The scripts only read the board
  and write copies under `RF Analysis/results/`. If you think the plan needs a change, tell the user.
- Ask before installing software other than what `Setup-and-Software.md` lists. Do not commit or push unless the user tells you to (see section 8 for results).
- Report faithfully: if a run fails or a number looks wrong, say so with the log lines. Do not tune settings until
  the result "looks right".
- Simulations run for tens of minutes to hours. Start them in the background with logging, and do not use a short
  command timeout.

**If a run is already in progress on this machine when you read this** (started before the `RF_TAG` folders
existed): let it finish. Do not pull, do not create `results/laptop/`, and do not start another simulation until it
is done. When it has finished, its output is in the old untagged path (for example
`RF Analysis/results/step1_long_coarse/`). Then: `git pull`, create `RF Analysis/results/laptop/`, move that
finished folder into it, and continue with the steps below. Do not delete or restart a running job.

## 3. Stage A: does a longer line fix the invalid S-parameters?

Script: `RF Analysis/sim/step1_ant3.py`. It crops a copy of the board to the ANT3 test line, replaces the SMA
connectors with two 0.32 mm test pads, and simulates the bare line between them with two microstrip ports.

Mode `long` uses a 9.5 mm section (y 26.5 to 36.0 mm). The 5 mm mode (`short`) already ran on the desktop and
gave the invalid result. A longer line tests the leading suspect: that the plugin's line ports need more line
between them than 5 mm.

1. Set up and check the toolchain (`Setup-and-Software.md` sections 3 and 4). Record the `MCells/s` figure.
2. Tell the scripts which machine this is, so results do not overwrite the desktop's (once per PowerShell window):

        $env:RF_TAG = "laptop"

   Results then go to `RF Analysis/results/laptop/`. The desktop uses `desktop`.
3. Dry run (builds the model, does not start the solver), from the repository root in PowerShell:

        $env:DRY = "1"
        & "C:\Program Files\KiCad\10.0\bin\python.exe" "RF Analysis\sim\step1_ant3.py" coarse long
        Remove-Item Env:DRY

   Expected: the script ends with `DRY run: model written, solver not started`. The model is about 134 x 110 x 39
   cells (about 0.58 million).
4. Run it (start in the background):

        & "C:\Program Files\KiCad\10.0\bin\python.exe" "RF Analysis\sim\step1_ant3.py" coarse long

   Expected time: about 30-35 minutes per port on the desktop, so about an hour or more in total on the
   desktop, less on the laptop. The two ports run one after the other.
5. Results are in `RF Analysis/results/laptop/step1_long_coarse/`. Read `summary.csv`, and the last 40 lines of
   `solver.log`.

**What good looks like:** the solver log has no line saying a port "gives out more power than it takes in"; S11 is
below about -15 dB and S21 is between about -0.5 and 0 dB across the L1 and L5 bands; the line impedance is
around 46-47 ohm at both ports.

**What to report:** the benchmark figure, the cells, timestep, steps done and wall-clock time for each port, the
line impedance at each port, whether the "more power than it takes in" warning appears, and `summary.csv`.

**If it still fails**, do not keep changing things blindly. Report it. The next diagnostics, in order, are: the
same 9.5 mm section at the medium mesh (`... step1_ant3.py medium long`; expect about 10 times the cost); the
plugin's own validation with a narrow 0.32 mm trace; and switching the port type. The user decides which.

## 4. Stage B: ANT1 chain, 4-port layout extraction (only after Stage A is reviewed)

Script: `RF Analysis/sim/step5_ant1_multiport.py`. It crops the board to the ANT1 chain (SMA end to the `U10`
`RF_IN` pad, about 12 mm) and builds a 4-port model:

| Port | Where | Type |
|---|---|---|
| 1 | test pad on the ANT1 trace just after the SMA `J7` (reference plane at y = 21.5 mm) | microstrip |
| 2 | `U10` pad 11 (`RF_IN_1`, the receiver input) | microstrip |
| 3 | `L2` pad 1 (bias-tee tap, on the RF line) | lumped |
| 4 | `L2` pad 2 (bias network side) | lumped |

Why `L2` is two ports and not a 56 nH lumped part: the plugin lowers the FDTD timestep by 0.7/sqrt(L in nH) for
any inductor. For 56 nH that is a factor of 0.094, about 10.7 times more timesteps, which is impractical. A
footprint that holds a port pad is skipped by the plugin, so `L2` is removed from the 3D model. The inductor is
added afterwards in a circuit simulation (section 6).

Everything else is kept as on the board: `C27` (100 pF DC block), `C30` (100 pF) and `C31` (100 nF) bias
decoupling, `R39` (10 ohm), `R35` (0 ohm series, a short), and the empty pi-filter pads `C28` and `C29`
(0 pF, treated as pads only). `D4` (the TVS) has no value in the model, so it is left open; its 0.05 pF is added in
the circuit stage. The connector launch (`J7`) is not part of this run.

Run order (from the repository root, PowerShell):

1. Dry run:

        $env:DRY = "1"
        & "C:\Program Files\KiCad\10.0\bin\python.exe" "RF Analysis\sim\step5_ant1_multiport.py" coarse
        Remove-Item Env:DRY

   Expected: four ports (`P1 Pad 1 (ANT1)`, `U10 Pad 11 (RF_IN_1)`, `L2 Pad 1`, `L2 Pad 2`) and the lumped parts
   `R35 R39 C27 C31 C30 C29 D4 C28`. `L2` must NOT be in the lumped list.
2. Trial with one excitation (port 1):

        & "C:\Program Files\KiCad\10.0\bin\python.exe" "RF Analysis\sim\step5_ant1_multiport.py" coarse 1

   The model is about 2.3 million cells (260 x 229 x 39). On the desktop's solver speed (about 35 million
   cells/s) expect roughly 2-3 hours per port there; the laptop should be faster.
3. If the trial is plausible (section 5), run ports 2, 3 and 4 the same way.

Results go to `RF Analysis/results/laptop/ant1_4port_coarse/` (with `RF_TAG` set to `laptop`).

## 5. Sanity checks on any result

- **The solver's own warning.** If the log says a port "gives out more power than it takes in", the S-parameters
  are not valid. Report it and stop.
- **Passivity:** the sum of squared S-parameter magnitudes in any column must not exceed 1 at any frequency.
- **Reciprocity:** S21 and S12 should agree once both ports have been excited.
- **Line impedance:** the value the plugin prints for the microstrip ports should be close to the desktop's
  (about 46-47 ohm at coarse mesh). A large difference means the geometry or stackup was misread.
- The lumped ports at `L2` (Stage B) have not been validated on this board.

## 6. What comes after Stage B (do only when asked)

1. Merge the per-port results into one 4-port Touchstone file.
2. In scikit-rf (installed in KiCad's Python) or Qucs-S, connect the missing parts to ports 3 and 4: `L2`
   (Murata LQW15AN56NG00D S-parameters, or an ideal 56 nH with its 2.8 GHz self-resonance), and the `D4` TVS
   (0.05 pF) at its own node. The bias network and supply side terminate port 4.
3. Read off S11 and S21 of the complete chain (port 1 to port 2) across 1.0 to 2.0 GHz and compare with the
   acceptance criteria in `RF-Analysis-Plan.md` section 7.

## 7. Known problems and tips

- KiCad's Python bindings hand back stale objects after a `Remove`. The scripts gather everything before removing
  and crop in a separate process. Keep that structure if you edit them.
- `step1_ant3.py` writes `solver.log` only when the solver exits (it does not flush). While it runs, the file
  `results/.../exc1/port_ut_1A` (last line, first column) shows the simulated time in seconds. The run ends when
  the energy criterion is met (about 2.1 ns of simulated time on the desktop for the 5 mm test) or at the
  300,000-step cap.
- The desktop and the laptop each need their own copy of the board file. After `git pull` the file
  `telemetry.kicad_pcb` will match the desktop's. Close KiCad on the laptop before pulling if the project is open.

## 8. Returning results to the repository

Only small result files go in git. `RF Analysis/results/.gitignore` already excludes the large solver output
(`exc*/` folders, the cropped `.kicad_pcb` copies, far-field files). What is kept: `results.s2p`, `summary.csv`,
`lines.json`, `model.json` and the solver logs (about 0.3 MB per run).

From the repository root, after a run finishes and only after the user says to:

    git pull
    git add "RF Analysis/results/laptop"
    git status
    git commit -m "RF results from laptop: <which run>"
    git push

Check `git status` before committing: no file over about 1 MB should be staged. The desktop uses its own folder
(`results/desktop`), so the two machines never edit the same files and there are no merge conflicts. If the user
prefers not to use git for results, copy the folder across by USB or a network share.

## 9. What to report back

- The `MCells/s` benchmark figure and the laptop model.
- For each run: cells, timestep, steps done, wall-clock time, whether the energy criterion was met.
- The results location, the sanity-check results, and any warnings from the log.
- Plots, if asked: S11 and S21 in the bands, and Smith charts.
