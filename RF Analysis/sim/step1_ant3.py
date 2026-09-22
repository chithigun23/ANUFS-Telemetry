"""Step 1 of RF-Analysis-Plan.md: simulate the bare ANT3 through-line (J9 -> J10).

Run with KiCad's Python (it needs pcbnew):
    "C:\\Program Files\\KiCad\\10.0\\bin\\python.exe" step1_ant3.py [coarse|medium|fine]

What it does:
  1. Copies telemetry.kicad_pcb, replaces the board outline with a small box
     around the ANT3 structure, and saves the copy in results/step1/.
     (The RFsim plugin fits its simulation domain to the whole board, so an
     uncropped run would simulate all 80x80 mm.)
  2. Builds the RFsim model with port 1 = J9 pad 1 and port 2 = J10 pad 1,
     both as microstrip ("msl") ports. This puts the reference planes at the
     SMA pads, so the connector launch itself is NOT part of this step (Step 2).
  3. Runs the openEMS solver from C:\\openEMS\\venv and writes a Touchstone file.
  4. Prints S11/S21 across the GNSS bands and saves a CSV summary.

Section modes ("short" or "long" as the 2nd argument): instead of the full 11 mm J9->J10
line, simulate a section of the same trace between two 0.32 mm test pads, in a tighter
domain. "short" = 5 mm (y 29.5 .. 34.5), "long" = 9.5 mm (y 26.5 .. 36.0). Same stackup, same ground pours, so the
impedance and the toolchain check are the same but the run is far cheaper.

The real board file is only read, never modified.
"""
import json
import os
import socket
import subprocess
import sys

import pcbnew
from pcbnew import FromMM, VECTOR2I

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
BOARD = os.path.join(REPO, "telemetry-kicad", "telemetry.kicad_pcb")
PLUGINS = os.environ.get("RFSIM_PLUGIN_DIR", r"C:\openEMS\kicad-rfsim\plugins")
sys.path.insert(0, PLUGINS)

import board_reader  # noqa: E402
import solverenv  # noqa: E402

# Results go to results/<TAG>/..., so two machines never write to the same path.
# Set RF_TAG (for example "desktop" or "laptop"); the default is the computer name.
TAG = os.environ.get("RF_TAG") or socket.gethostname()

# Crop box around ANT3 (mm, KiCad coordinates): the two SMA pads are at
# x = 18.29, y = 25.96 and y = 37.29.
BOX = (10.0, 20.0, 27.0, 43.0)  # x0, y0, x1, y1
# Section modes: crop box (x0, y0, x1, y1) and the y of test pad 1 and test pad 2 (mm).
SECTIONS = {
    "short": ((14.0, 27.0, 22.5, 37.0), (34.5, 29.5)),   # 5 mm
    "long": ((14.0, 26.0, 22.5, 37.0), (35.0, 28.0)),    # 7 mm; ends kept on the inner-plane copper (the SMA clearance holes reach y = 35.8 and 27.5)
    # Diagnostic (Next-Steps-Sonnet.md Task E prep): isolate length from absolute position. Port 1
    # held at "short"'s own working y=34.5; port 2 moved out to 6 mm to see if a length threshold
    # or a location-specific defect explains the 7 mm section's near-total-reflection anomaly.
    "probe6mm": ((14.0, 26.0, 22.5, 37.0), (34.5, 28.5)),
}
TRACE_X = 18.26
BANDS = {"L5": (1164e6, 1188e6), "L1": (1559e6, 1606e6)}


def make_cropped_board(out_path, section=None):
    short = section is not None
    b = pcbnew.LoadBoard(BOARD)
    for d in list(b.GetDrawings()):
        if d.GetLayer() == pcbnew.Edge_Cuts:
            b.Remove(d)
    x0, y0, x1, y1 = SECTIONS[section][0] if short else BOX
    pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    for i in range(4):
        seg = pcbnew.PCB_SHAPE(b)
        seg.SetShape(pcbnew.SHAPE_T_SEGMENT)
        seg.SetLayer(pcbnew.Edge_Cuts)
        seg.SetStart(VECTOR2I(FromMM(pts[i][0]), FromMM(pts[i][1])))
        seg.SetEnd(VECTOR2I(FromMM(pts[(i + 1) % 4][0]), FromMM(pts[(i + 1) % 4][1])))
        seg.SetWidth(FromMM(0.05))
        b.Add(seg)
    # Bare-line baseline: keep only the two SMA connectors, the ANT3/GND
    # copper and the ground pours. Everything else on the board (the ANT1
    # filter parts, unrelated tracks and vias) is deleted from this COPY so
    # it cannot add mesh lines or lumped elements to the simulation.
    tracks = b.Tracks()
    ant3_tracks = [tracks[i] for i in range(tracks.size()) if tracks[i].GetNetname() == "ANT3"]
    ant3_code = ant3_tracks[0].GetNetCode()
    doomed = [tracks[i] for i in range(tracks.size()) if tracks[i].GetNetname() not in ("ANT3", "GND")]
    for tr in doomed:
        b.Remove(tr)
    keep = () if short else ("J9", "J10")
    for fp in [f for f in b.GetFootprints() if f.GetReference() not in keep]:
        b.Remove(fp)
    if short:
        # Replace the ANT3 track with a 5 mm section and give it a test pad at each end.
        for tr in ant3_tracks:
            b.Remove(tr)
        trk = pcbnew.PCB_TRACK(b)
        trk.SetLayer(pcbnew.F_Cu)
        trk.SetNetCode(ant3_code)
        trk.SetWidth(FromMM(0.32))
        trk.SetStart(VECTOR2I(FromMM(TRACE_X), FromMM(SECTIONS[section][1][0])))
        trk.SetEnd(VECTOR2I(FromMM(TRACE_X), FromMM(SECTIONS[section][1][1])))
        b.Add(trk)
        for ref, y in (("P1", SECTIONS[section][1][0]), ("P2", SECTIONS[section][1][1])):
            fp = pcbnew.FOOTPRINT(b)
            fp.SetReference(ref)
            fp.SetPosition(VECTOR2I(FromMM(TRACE_X), FromMM(y)))
            pad = pcbnew.PAD(fp)
            pad.SetNumber("1")
            pad.SetShape(pcbnew.PAD_SHAPE_RECT)
            pad.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
            pad.SetLayerSet(pad.SMDMask())
            pad.SetSize(VECTOR2I(FromMM(0.32), FromMM(0.32)))
            pad.SetPosition(fp.GetPosition())
            pad.SetNetCode(ant3_code)
            fp.Add(pad)
            b.Add(fp)
    pcbnew.SaveBoard(out_path, b)


def main(mesh="coarse", mode="full"):
    short = mode in SECTIONS
    outdir = os.path.join(REPO, "RF Analysis", "results", TAG, "step1_%s_%s%s" % (mode, mesh, os.environ.get("SUFFIX", "")))
    os.makedirs(outdir, exist_ok=True)
    cropped = os.path.join(outdir, "ant3_cropped.kicad_pcb")
    # Crop in a separate process: KiCad's Python bindings misbehave when a board
    # is edited and another board is loaded in the same process.
    subprocess.check_call([sys.executable, os.path.abspath(__file__), "--crop", cropped, mode])

    board = pcbnew.LoadBoard(cropped)
    refs = ("P1", "P2") if short else ("J9", "J10")
    pads = [board.FindFootprintByReference(r).FindPadByNumber("1") for r in refs]
    margin = 2.0 if short else 4.0
    model = board_reader.extract(board, pads, margin_mm=margin)
    # PORTTYPE=lumped uses lumped ports (no wave separation along a line), which suits
    # structures that are electrically short at GNSS frequencies. Default is msl.
    for p in model["ports"]:
        p["type"] = os.environ.get("PORTTYPE", "msl")
    model["settings"] = {
        "f_start": 0.5e9, "f_stop": 3.0e9, "z0": 50.0, "margin_mm": margin,
        "mesh": mesh, "n_freq": 251, "max_timesteps": 300000,
        "end_criteria": 1e-4,
    }
    # EXCITE="1" excites port 1 only: a two-port through line needs just that run
    # for S11 and S21, which halves the cost. Default is both ports.
    if os.environ.get("EXCITE"):
        model["settings"]["excite"] = [int(x) for x in os.environ["EXCITE"].split(",")]
    # Diagnostics for the late-time instability: TSF scales the FDTD timestep (below 1
    # is smaller, more stable, slower); MAXSTEPS raises the step cap to match.
    if os.environ.get("TSF"):
        model["settings"]["time_step_factor"] = float(os.environ["TSF"])
    if os.environ.get("MAXSTEPS"):
        model["settings"]["max_timesteps"] = int(os.environ["MAXSTEPS"])
    if os.environ.get("ENDCRIT"):
        model["settings"]["end_criteria"] = float(os.environ["ENDCRIT"])
    model_path = os.path.join(outdir, "model.json")
    with open(model_path, "w") as fh:
        json.dump(model, fh, indent=1)
    if os.environ.get("DRY"):
        print("DRY run: model written, solver not started")
        return
    print("stackup source:", model.get("stackup_source", "?"))
    print("ports:", [(p["label"], p["direction"], p["track_width"]) for p in model["ports"]])

    runner = os.path.join(HERE, os.environ["RUNNER"]) if os.environ.get("RUNNER") else os.path.join(PLUGINS, "runner.py")
    solver_py = solverenv.solver_python() or sys.executable
    with open(os.path.join(outdir, "solver.log"), "w") as log:
        proc = subprocess.Popen([solver_py, runner, model_path, outdir],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        for line in proc.stdout:
            log.write(line)
            if line.startswith("[rfsim]") or "warning" in line.lower():
                print(line.rstrip())
        if proc.wait() != 0:
            raise SystemExit("solver failed, see solver.log")

    import numpy as np
    s2p = os.path.join(outdir, "results.s2p")
    rows = np.loadtxt(s2p, comments=("!", "#"))
    f = rows[:, 0]
    s11 = 20 * np.log10(np.abs(rows[:, 1] + 1j * rows[:, 2]) + 1e-12)
    s21 = 20 * np.log10(np.abs(rows[:, 3] + 1j * rows[:, 4]) + 1e-12)
    with open(os.path.join(outdir, "summary.csv"), "w") as fh:
        fh.write("band,f_start_hz,f_stop_hz,S11_worst_dB,S21_worst_dB\n")
        for name, (a, b_) in BANDS.items():
            m = (f >= a) & (f <= b_)
            fh.write("%s,%g,%g,%.2f,%.2f\n" % (name, a, b_, s11[m].max(), s21[m].min()))
            print("%s: S11 worst %.1f dB, S21 worst %.2f dB" % (name, s11[m].max(), s21[m].min()))


if __name__ == "__main__":
    if sys.argv[1:2] == ["--crop"]:
        make_cropped_board(sys.argv[2], section=sys.argv[3] if sys.argv[3:4] and sys.argv[3] in SECTIONS else None)
    else:
        main(*(sys.argv[1:] or ["coarse"]))
