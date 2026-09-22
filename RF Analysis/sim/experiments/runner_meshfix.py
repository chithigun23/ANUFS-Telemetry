"""EXPERIMENTAL VARIANT of the RFsim plugin's runner.py (MIT licence, github.com/RolandWa/kicad-rfsim).

Task B item 3 (Next-Steps-Sonnet.md section 3). Not for production results -- a candidate fix to test
in Task C/E, run through run_variant.py's machinery.

What it changes, and why: runner.py's _mesh() already grades the mesh across a "msl"/"cpw"/"stripline"
port's strip width (see runner.py around line 433, "A stripline and a microstrip need..."), but a
"lumped" port -- what every one of our board runs actually uses -- gets NO such treatment (see
_port_geometry, the `else` branch that sets g["type"] = "lumped"). Measured with mesh_check.py: the
0.32 mm ANT3 trace gets only 2 mesh cells across it, at BOTH the coarse and the fine preset, because
the port box is the only thing anchoring the mesh there. This module inserts extra x/y anchor lines
across every lumped port's own box (>= MIN_CELLS subdivisions on each axis) before the plugin's own
smoothing runs, the same idea runner.py already applies to msl/cpw/stripline ports, generalised to
lumped ports.

This does NOT touch the open-air neighbour-cell-ratio jump mesh_check.py also found (fine mesh:
worst ratio 4-10x; coarse: 15-36x, concentrated just above the board). That is a separate, larger
change (bounding SmoothMeshLines's local ratio near the board specifically) left for a later pass if
Task C/E still shows instability after this fix.
"""
import os
import sys

import numpy as np

PLUGINS = os.environ.get("RFSIM_PLUGIN_DIR", r"C:\openEMS\kicad-rfsim\plugins")
sys.path.insert(0, PLUGINS)
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import runner  # noqa: E402

MIN_CELLS = 3
_orig_mesh = runner._mesh


def _add_snapped(vals, candidate, merge_dist):
    """Add `candidate`, unless an existing line already sits within `merge_dist` of it, in which
    case do nothing (that existing line already does the job this candidate would have done).

    A plain distance check against a single global tolerance does not work here: two different
    failure modes showed up on the same port box. A tolerance big enough to reject a candidate
    ~0.053 mm from the port's own pre-existing centre anchor (so it doesn't get rejected outright,
    losing the subdivision) is also too small to catch a candidate that lands ~0.006 mm from an
    unrelated nearby anchor, which leaves a pathological sliver cell (measured: y worst ratio 9.5x
    -> 26x). So `merge_dist` is sized relative to the CELL this candidate is trying to create
    (half the target spacing for this port), not a fixed fraction of the FDTD resolution `res`."""
    for v in vals:
        if abs(candidate - v) < merge_dist:
            return
    vals.add(candidate)


def _refine_lumped_ports(xs, ys, ports):
    xs, ys = set(xs), set(ys)
    for g in ports:
        if g.get("type") != "lumped":
            continue  # msl/cpw/stripline already graded by the plugin
        x0, x1 = sorted((g["start"][0], g["stop"][0]))
        y0, y1 = sorted((g["start"][1], g["stop"][1]))
        if x1 > x0:
            merge_dist = 0.5 * (x1 - x0) / MIN_CELLS
            for c in np.linspace(x0, x1, MIN_CELLS + 1):
                _add_snapped(xs, float(c), merge_dist)
        if y1 > y0:
            merge_dist = 0.5 * (y1 - y0) / MIN_CELLS
            for c in np.linspace(y0, y1, MIN_CELLS + 1):
                _add_snapped(ys, float(c), merge_dist)
    return xs, ys


def _mesh(model, ports, res):
    xs, ys, zs = _orig_mesh(model, ports, res)
    xs, ys = _refine_lumped_ports(xs, ys, ports)
    return sorted(xs), sorted(ys), zs


runner._mesh = _mesh

if __name__ == "__main__":
    runner.main(sys.argv[1], sys.argv[2])
