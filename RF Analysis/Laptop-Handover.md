# Handover: ANT1 chain 4-port simulation (for the laptop)

This file is written for a Claude Code instance (or a person) picking up work on a second machine. Read it fully
before running anything. Background is in `RF-Analysis-Plan.md`; software setup is in `Setup-and-Software.md`.

## 1. Context in five lines

- Project: ANUFS-Telemetry, a KiCad 10 PCB with two GNSS receivers (LC29H) fed by active antennas.
- Concern: the bias-tee, TVS, DC-block capacitor and pi filter on each antenna line might cause reflections that
  degrade the signal at the receiver's `RF_IN` pin (L1 1559-1606 MHz, L5 1164-1188 MHz).
- Method: 3D full-wave simulation (openEMS, FDTD) of the real PCB layout, using the RFsim plugin's board reader and
  solver runner from KiCad data.
- The desktop is already running Step 1 (a bare 5 mm section of the ANT3 test line, see `Short-Section-Test.md`).
- This machine's job: the ANT1 chain, as a 4-port layout extraction (section 3).

## 2. Rules for this task

- Do not modify `telemetry.kicad_pcb`, the schematic, or `RF-Analysis-Plan.md`. The scripts only read the board
  and write copies under `RF Analysis/results/`. If you think the plan needs a change, tell the user.
- Ask before installing software other than what `Setup-and-Software.md` lists. Do not commit or push.
- Report faithfully: if a run fails or a number looks wrong, say so with the log lines. Do not tune settings until
  the result "looks right".
- Simulations run for hours. Start them in the background with logging, and do not use a short command timeout.

## 3. The task

Script: `RF Analysis/sim/step5_ant1_multiport.py`. It crops a copy of the board to the ANT1 chain (SMA end to the
`U10` `RF_IN` pad, about 12 mm) and builds a 4-port model:

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

## 4. Run order

Set up first (`Setup-and-Software.md` section 3), then check the toolchain (its section 4) and record the
`MCells/s` figure. Then, from the repository root, in PowerShell:

1. Dry run, builds the model but does not start the solver:

        $env:DRY = "1"
        & "C:\Program Files\KiCad\10.0\bin\python.exe" "RF Analysis\sim\step5_ant1_multiport.py" coarse
        Remove-Item Env:DRY

   Expected: the four ports listed as `P1 Pad 1 (ANT1)`, `U10 Pad 11 (RF_IN_1)`, `L2 Pad 1`, `L2 Pad 2`, and the
   lumped parts `R35 R39 C27 C31 C30 C29 D4 C28`. `L2` must NOT appear in the lumped list.

2. Trial: one excitation only (port 1). Start it in the background and watch the log:

        & "C:\Program Files\KiCad\10.0\bin\python.exe" "RF Analysis\sim\step5_ant1_multiport.py" coarse 1

   The model is about 2.3 million cells (260 x 229 x 39). The desktop solver does about 32 million cells/s, so
   expect about 6 hours per port on the desktop and roughly 2-4 hours per port on this laptop. Check the log after
   a few minutes: the `Speed` and `Timestep` lines give the rate and the number of steps done.

3. If the trial finishes and the S-parameters are plausible (section 5), run ports 2, 3 and 4 the same way
   (`... coarse 2`, and so on). Each writes its own log, `solver_<ports>.log`.

Results go to `RF Analysis/results/ant1_4port_coarse/`.

## 5. Sanity checks on the result

- Passivity: the sum of squared S-parameter magnitudes in any column must not exceed 1 at any frequency.
- Reciprocity: S21 and S12 should agree once both ports have been excited.
- The line impedance the plugin prints for the microstrip ports should be close to the Step 1 result (about
  50-54 ohm expected). A large difference means the geometry or stackup was misread.
- Lumped-port results at ports 3 and 4 have not been validated on this board. Treat them as unproven until they
  pass the checks above, and say so in your report.

## 6. What comes after the solver runs (do only when asked)

1. Merge the per-port results into one 4-port Touchstone file.
2. In scikit-rf (already installed in KiCad's Python) or Qucs-S, connect the missing parts to ports 3 and 4:
   `L2` (Murata LQW15AN56NG00D S-parameters, or an ideal 56 nH with its 2.8 GHz self-resonance), and the `D4`
   TVS (0.05 pF) at its own node. The bias network and supply side terminate ports 4.
3. Read off S11 and S21 of the complete chain (port 1 to port 2) across 1.0 to 2.0 GHz and compare with the
   acceptance criteria in `RF-Analysis-Plan.md` section 7.

## 7. Known problems and tips

- KiCad's Python bindings hand back stale objects after a `Remove`. The script gathers everything before removing
  and crops in a separate process. Keep that structure if you edit it.
- The solver log is only written to disk when the solver exits, unless the script flushes it (this one does).
  `results/.../exc1/` fills as the run progresses and shows it is alive.
- The default 300,000-step cap gives a warning that it is under three times the excitation length. If a run stops
  at the cap without the energy criterion being met, report it; do not silently raise the cap.
- The desktop and the laptop each need their own copy of the board file. `telemetry.kicad_pcb` has uncommitted
  changes on the desktop (the JLCPCB stackup values and routing edits), so a plain `git pull` will not bring them
  across. The user will copy the file or commit it.

## 8. What to report back

- The `MCells/s` benchmark figure and the laptop model.
- For each run: cells, timestep, steps done, wall-clock time, whether the energy criterion was met.
- The four-port Touchstone file location, the sanity-check results, and any warnings from the log.
- Plots, if asked: S11 and S21 in the bands, and Smith charts.
