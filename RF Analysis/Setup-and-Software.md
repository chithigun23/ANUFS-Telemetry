# RF Analysis — What Is Being Done and What Software You Need

Companion to `RF-Analysis-Plan.md`. This file covers the software stack, how it was set up on the desktop, how
to set it up on another machine (the laptop), and whether the laptop is worth using for the simulations.

## 1. What is being done

The goal (see the plan) is to find out whether the bias-tee, TVS, DC block and pi filter on the GNSS antenna
lines cause reflections that degrade the signal. The method is 3D electromagnetic simulation of the RF section
of the real PCB with openEMS, driven from KiCad data.

The pipeline, as built:

1. A script (`sim/step1_ant3.py`) reads `telemetry.kicad_pcb` with KiCad's Python and writes a cropped copy with
   only the parts relevant to the test. The real board file is never modified.
2. The RFsim plugin's board reader converts that copy (stackup, copper polygons, vias, ports, lumped parts) into
   a model file.
3. The plugin's solver script runs openEMS (FDTD) on it and writes S-parameters (a Touchstone `.s2p` file).
4. The script summarises S11 and S21 in the L5 (1164-1188 MHz) and L1 (1559-1606 MHz) bands.

The steps in the plan build on each other. Step 1 is the bare line (running now, see `Short-Section-Test.md`).
Later steps add the connector, then each filter and bias part, one at a time.

## 2. Software

| Software | Version installed | Purpose | Source |
|---|---|---|---|
| KiCad | 10.0 | reads the board; its Python (with `pcbnew`) builds the model | kicad.org |
| openEMS | 0.37.0-rc2 (Windows x64, MSVC build) | the FDTD solver | [openEMS-Project releases](https://github.com/thliebig/openEMS-Project/releases), file `openEMS_x64_v0.37.0-rc2_msvc.zip` (62.6 MB) |
| Python | 3.14.7 | runs the solver (openEMS ships wheels for 3.13 and 3.14) | Python install manager: `py install 3.14` |
| CSXCAD, openEMS Python packages | 0.7.0rc2 / 0.37.0rc2 | Python interface to the solver | wheels inside the openEMS zip (`C:\openEMS\python`) |
| scikit-rf, matplotlib, h5py | pip latest | plotting and Touchstone handling inside KiCad's Python | `pip install --user` into KiCad's Python |
| RFsim (kicad-rfsim) | 1.1.0 | board reader and solver runner; also a KiCad toolbar plugin | [RolandWa/kicad-rfsim](https://github.com/RolandWa/kicad-rfsim) (MIT licence); no published release, so used from source |

Not needed: the KiCad plugin button is optional. The scripts in `sim/` call the plugin's board reader and runner
directly, so the plugin does not have to be installed into KiCad. It only matters if you want to click pads and
run simulations from the PCB editor.

Not used: the graphics card. openEMS has no GPU support in its standard build.

## 3. Setup from scratch (Windows)

Do these in order. Adjust paths if you install elsewhere.

1. **KiCad 10.0**: install the standard build, which includes its own Python 3.11.
2. **Python 3.14** (in PowerShell): `py install 3.14`
3. **openEMS**: download `openEMS_x64_v0.37.0-rc2_msvc.zip` from the releases page and extract it so the folder
   `C:\openEMS` contains `openEMS.exe` and the `python` folder.
4. **Solver environment** (in PowerShell):

        py -3.14 -m venv C:\openEMS\venv
        C:\openEMS\venv\Scripts\python.exe -m pip install --find-links C:\openEMS\python csxcad openems
        C:\openEMS\venv\Scripts\python.exe -c "import os; os.add_dll_directory('C:/openEMS'); import CSXCAD, openEMS; print('ok')"

   The last command must print `ok`. A warning about the HDF5 version is harmless.
5. **KiCad's Python packages**:

        "C:\Program Files\KiCad\10.0\bin\python.exe" -m pip install --user scikit-rf matplotlib h5py

6. **RFsim source**: `git clone https://github.com/RolandWa/kicad-rfsim C:\openEMS\kicad-rfsim`
   The scripts expect the plugin code in `C:\openEMS\kicad-rfsim\plugins`. To use another folder, set the
   `RFSIM_PLUGIN_DIR` environment variable.
7. **This repository**: `git clone` or pull `ANUFS-Telemetry` so `RF Analysis\sim\step1_ant3.py` exists.
8. **Optional, the toolbar plugin**: build a zip containing `metadata.json`, `plugins/` and `resources/` from the
   RFsim repository, then in KiCad use Plugin and Content Manager, Install from File.

## 4. Check the setup works

Run the plugin's own validation. It simulates a known 50 ohm microstrip and takes under a minute:

    cd C:\openEMS\kicad-rfsim\validation
    "C:\Program Files\KiCad\10.0\bin\python.exe" run_headless.py coarse

Expected end of the output: `S11 max: -11.0 dB   S21 min: -0.3 dB` and `PASS`. On the desktop the log line
`Speed: 19.28 MCells/s` was printed. Note that number, it is the benchmark used in section 5.

Then run the first real test (dry run first, to check the model builds without starting a long simulation):

    $env:DRY = "1"
    & "C:\Program Files\KiCad\10.0\bin\python.exe" "RF Analysis\sim\step1_ant3.py" coarse short
    Remove-Item Env:DRY
    & "C:\Program Files\KiCad\10.0\bin\python.exe" "RF Analysis\sim\step1_ant3.py" coarse short

(PowerShell syntax; run from the repository root.)

## 5. Is the laptop worth using?

Your laptop: Intel Core Ultra 7 258V (Lunar Lake), 32 GB RAM, Arc 140V graphics. The desktop: AMD Ryzen 3 3100
(4 cores, 8 threads), 32 GB DDR4-3200.

Short answer: **probably yes, as a second machine, with a modest speed-up. Do not buy anything.** Everything below
is an estimate; the benchmark in this section will settle it.

| Factor | Desktop (Ryzen 3 3100) | Laptop (Core Ultra, Lunar Lake) | Effect on openEMS |
|---|---|---|---|
| Memory bandwidth (theoretical) | about 51 GB/s (dual-channel DDR4-3200) | about 136 GB/s (on-package LPDDR5X-8533) | **the main factor**: FDTD is memory-bandwidth bound |
| Cores | 4 cores, 8 threads | 4 performance + 4 efficiency cores, 8 threads | similar thread count; the laptop's cores are much newer and faster per core |
| GPU | none used | Arc 140V not used | no help; openEMS has no GPU support |
| Cooling | desktop, sustained | thin laptop, can throttle on long runs | runs are hours long, so sustained performance matters |

Estimate: the memory bandwidth is about 2.7 times higher on paper. Real FDTD speed-ups from bandwidth are usually
smaller, so plan on **about 1.5 to 2.5 times faster**, not 10 times. A run of 50 minutes on the desktop would take
about 20 to 35 minutes on the laptop.

What that means in practice:
- Worth it for the run schedule: the plan has many simulations (each step, both chains), so a 2x speed-up helps,
  and having two machines lets you run two simulations at once.
- Not a fix for the full-line run: a full-line run at 5-7 hours per port would still be 2-4 hours per port.
  The short-section and cropped-domain approach is what makes the work practical, not the hardware.
- Keep the laptop plugged in, set Windows to the Best performance power mode, and check that it does not throttle
  during a long run (watch the log's cells-per-second figure over time).

**How to find out for certain (5 minutes):** after setup, run the validation in section 4 on the laptop and compare
the `MCells/s` figure with the desktop's 19.28. If the laptop is above about 30, it is clearly worth using. If it
is close to 19, keep using the desktop and do not bother moving files across.

## 6. Notes and caveats

- openEMS 0.37.0-rc2 is a release candidate. The plugin needs 0.37 or later for lumped inductors, which the
  bias tee and filter simulations will use.
- The simulation results depend on the stackup. The KiCad file now uses JLCPCB's published values, but the
  loss tangent (0.02) is a generic assumption.
- Coarse-mesh results read slightly low in impedance. Treat them as a screening tool, then rerun key cases at the
  medium preset.
- The plugin is new and has a single author. The validation run in section 4 is the main safeguard, and the
  planned VNA measurement of the ANT3 line (plan, Step 6) is the real check.
