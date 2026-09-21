"""ANT1 chain, 4-port layout extraction (a look-ahead of plan Steps 3-5).

Run with KiCad's Python (needs pcbnew):
    "C:\\Program Files\\KiCad\\10.0\\bin\\python.exe" step5_ant1_multiport.py [coarse|medium|fine] [excite]

    excite  optional comma list of ports to excite, e.g. 1,2 (default: all).
            Each excited port is one full solver run, so ports can be split
            between machines and the .s4p files merged afterwards.

Ports (all on the real ANT1 layout, cropped from telemetry.kicad_pcb):
    1  port on a test pad on the ANT1 trace, just after the SMA J7
    2  port on the LC29H RF_IN pad (U10 pad 11)
    3  port on L2 pad 1 (the bias-tee tap, ANT1 side)
    4  port on L2 pad 2 (the bias network side)
    All four are lumped ports by default (PORTTYPE=msl makes ports 1 and 2 microstrip ports).
    Use the FINE mesh: the coarse and medium presets make the solver unstable on this stack-up.

Why L2 is a pair of ports and not a lumped 56 nH part: the plugin lowers the
FDTD timestep as 0.7 / sqrt(L in nH), which for 56 nH is a factor of 0.094,
about 10.7 times more timesteps. Instead the layout is simulated with L2
removed (a footprint that holds a port pad is skipped by the plugin) and the
inductor is added afterwards in a circuit simulator from its S-parameters.
All other parts (C27 DC block, C30/C31, R35, R39, the empty pi-filter pads C28
and C29) are simulated as lumped parts as they are on the board. D4 (the TVS)
has no value in the schematic model, so the plugin leaves it open; its 0.05 pF
is added in the circuit stage.

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

BOX = (21.0, 20.5, 31.5, 33.0)          # x0, y0, x1, y1 (mm, KiCad coordinates)
TRACE_X, TEST_Y = 25.89, 21.5           # test pad on the ANT1 trace, above J7
KEEP_REFS = {"C27", "C28", "C29", "C30", "C31", "D4", "L2", "R35", "R39", "U10"}
KEEP_NETS = {"ANT1", "Net-(C27-Pad2)", "RF_IN_1", "Net-(C30-Pad1)", "GND", "VDD_RF_1"}


def make_cropped_board(out_path):
    b = pcbnew.LoadBoard(BOARD)
    for d in list(b.GetDrawings()):
        if d.GetLayer() == pcbnew.Edge_Cuts:
            b.Remove(d)
    x0, y0, x1, y1 = BOX
    pts = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
    for i in range(4):
        seg = pcbnew.PCB_SHAPE(b)
        seg.SetShape(pcbnew.SHAPE_T_SEGMENT)
        seg.SetLayer(pcbnew.Edge_Cuts)
        seg.SetStart(VECTOR2I(FromMM(pts[i][0]), FromMM(pts[i][1])))
        seg.SetEnd(VECTOR2I(FromMM(pts[(i + 1) % 4][0]), FromMM(pts[(i + 1) % 4][1])))
        seg.SetWidth(FromMM(0.05))
        b.Add(seg)

    # Gather everything first: the bindings hand back stale objects after a Remove.
    tracks = b.Tracks()
    all_tracks = [tracks[i] for i in range(tracks.size())]
    ant1 = [t for t in all_tracks if t.GetNetname() == "ANT1"]
    ant1_code = ant1[0].GetNetCode()
    doomed = [t for t in all_tracks if t.GetNetname() not in KEEP_NETS]
    # ANT1 track below the test pad (towards J7) is removed; the port sits at its end.
    below = [t for t in ant1 if type(t).__name__ == "PCB_TRACK"
             and max(pcbnew.ToMM(t.GetStart().y), pcbnew.ToMM(t.GetEnd().y)) <= TEST_Y + 0.01
             or (type(t).__name__ == "PCB_TRACK"
                 and pcbnew.ToMM(t.GetStart().y) < TEST_Y and pcbnew.ToMM(t.GetEnd().y) > TEST_Y)]
    for t in doomed + below:
        b.Remove(t)

    trk = pcbnew.PCB_TRACK(b)
    trk.SetLayer(pcbnew.F_Cu)
    trk.SetNetCode(ant1_code)
    trk.SetWidth(FromMM(0.32))
    trk.SetStart(VECTOR2I(FromMM(TRACE_X), FromMM(TEST_Y)))
    trk.SetEnd(VECTOR2I(FromMM(TRACE_X), FromMM(23.62)))
    b.Add(trk)

    fp = pcbnew.FOOTPRINT(b)
    fp.SetReference("P1")
    fp.SetPosition(VECTOR2I(FromMM(TRACE_X), FromMM(TEST_Y)))
    pad = pcbnew.PAD(fp)
    pad.SetNumber("1")
    pad.SetShape(pcbnew.PAD_SHAPE_RECT)
    pad.SetAttribute(pcbnew.PAD_ATTRIB_SMD)
    pad.SetLayerSet(pad.SMDMask())
    pad.SetSize(VECTOR2I(FromMM(0.32), FromMM(0.32)))
    pad.SetPosition(fp.GetPosition())
    pad.SetNetCode(ant1_code)
    fp.Add(pad)
    b.Add(fp)

    for f in [f for f in b.GetFootprints() if f.GetReference() not in KEEP_REFS | {"P1"}]:
        b.Remove(f)
    pcbnew.SaveBoard(out_path, b)


def main(mesh="coarse", excite=None):
    outdir = os.path.join(REPO, "RF Analysis", "results", TAG, "ant1_4port_" + mesh)
    os.makedirs(outdir, exist_ok=True)
    cropped = os.path.join(outdir, "ant1_cropped.kicad_pcb")
    # Crop in a separate process: the bindings misbehave when a board is edited
    # and another is loaded in the same process.
    subprocess.check_call([sys.executable, os.path.abspath(__file__), "--crop", cropped])

    board = pcbnew.LoadBoard(cropped)
    l2 = board.FindFootprintByReference("L2")
    u10 = board.FindFootprintByReference("U10")
    rf_in = [p for p in u10.Pads() if p.GetNetname() == "RF_IN_1"][0]
    pads = [board.FindFootprintByReference("P1").FindPadByNumber("1"), rf_in,
            l2.FindPadByNumber("1"), l2.FindPadByNumber("2")]
    margin = 3.0
    model = board_reader.extract(board, pads, margin_mm=margin)
    # Lumped ports for all four: the microstrip-port wave separation gave invalid S-parameters on
    # the short 0.32 mm line (see Short-Section-Test.md). PORTTYPE=msl restores microstrip ports 1 and 2.
    kinds = ("msl", "msl", "lumped", "lumped") if os.environ.get("PORTTYPE") == "msl" else ("lumped",) * 4
    for p, kind in zip(model["ports"], kinds):
        p["type"] = kind
    # Unpopulated pads: the plugin models a value of 0 as a series RL (0.25 nH to ground), which
    # is a near-short across the RF line at GNSS frequencies. An empty pad is an open circuit, so
    # these parts are dropped from the lumped-element list (the pad copper stays).
    empty = {"C28", "C29"}
    model["lumped_elements"] = [e for e in model.get("lumped_elements", []) if e["ref"] not in empty]
    settings = {
        "f_start": 0.5e9, "f_stop": 3.0e9, "z0": 50.0, "margin_mm": margin,
        "mesh": mesh, "n_freq": 251, "max_timesteps": 300000,
        "end_criteria": 1e-4,
    }
    if excite:
        settings["excite"] = [int(x) for x in excite.split(",")]
    model["settings"] = settings
    model_path = os.path.join(outdir, "model.json")
    with open(model_path, "w") as fh:
        json.dump(model, fh, indent=1)
    print("ports:", [(p["label"], p["type"], p["direction"]) for p in model["ports"]])
    print("lumped parts kept:", [(e["ref"], e["type"], e["value"]) for e in model.get("lumped_elements", [])])
    if os.environ.get("DRY"):
        print("DRY run: model written, solver not started")
        return

    runner = os.path.join(PLUGINS, "runner.py")
    solver_py = solverenv.solver_python() or sys.executable
    tag = (excite or "all").replace(",", "")
    with open(os.path.join(outdir, "solver_%s.log" % tag), "w") as log:
        proc = subprocess.Popen([solver_py, runner, model_path, outdir],
                                stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
        for line in proc.stdout:
            log.write(line)
            log.flush()
            if line.startswith("[rfsim]") or "warning" in line.lower():
                print(line.rstrip())
        if proc.wait() != 0:
            raise SystemExit("solver failed, see the log")
    print("done; Touchstone file(s) in", outdir)


if __name__ == "__main__":
    if sys.argv[1:2] == ["--crop"]:
        make_cropped_board(sys.argv[2])
    else:
        main(*(sys.argv[1:] or ["coarse"]))
