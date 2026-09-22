"""Task B item 1 (Next-Steps-Sonnet.md section 3): report the actual FDTD mesh for a model.json.

Reuses the plugin's own mesh-building code (runner.py: _port_geometry, _mesh, SmoothMeshLines) so the
numbers reported here are exactly the mesh a real run would use, not a re-implementation that could
disagree with it.

Run: "C:\\Program Files\\KiCad\\10.0\\bin\\python.exe" mesh_check.py <model.json> [model.json ...]
"""
import json
import os
import sys

import numpy as np

PLUGINS = os.environ.get("RFSIM_PLUGIN_DIR", r"C:\openEMS\kicad-rfsim\plugins")
sys.path.insert(0, PLUGINS)
import runner  # noqa: E402
from CSXCAD.SmoothMeshLines import SmoothMeshLines  # noqa: E402


def actual_mesh_lines(model):
    """The exact three (x, y, z) line arrays a real run of this model would use."""
    s = model["settings"]
    eps_max = max(d["epsilon"] for d in model["dielectric_layers"])
    # Use runner.C0 (m/s) directly, exactly as runner.py's own main() does -- redefining the
    # constant with different units here once caused a 1000x error in `res` that silently
    # disabled all mesh smoothing (see git history of this file).
    lam_min = runner.C0 / s["f_stop"] / np.sqrt(eps_max) * 1e3
    res = lam_min / runner.RES_DIV[s["mesh"]]
    ports_geo = runner._port_geometry(model, res)
    raw = runner._mesh(model, ports_geo, res)
    return [np.round(SmoothMeshLines(lines, res, 1.4), 9) for lines in raw], res


def cell_stats(lines):
    lines = np.sort(np.asarray(lines, dtype=float))
    d = np.diff(lines)
    d = d[d > 0]
    if len(d) == 0:
        return None
    ratio = np.maximum(d[1:] / d[:-1], d[:-1] / d[1:])
    return {
        "n_lines": len(lines), "n_cells": len(d),
        "min_cell": float(d.min()), "max_cell": float(d.max()),
        "worst_ratio": float(ratio.max()) if len(ratio) else 1.0,
        "cells": d,
        "line_values": lines,
    }


def cells_across(lines, lo, hi):
    """How many cells fall strictly inside [lo, hi] (e.g. a trace width or a dielectric layer)."""
    lines = np.sort(np.asarray(lines, dtype=float))
    inside = lines[(lines >= lo - 1e-9) & (lines <= hi + 1e-9)]
    return max(0, len(inside) - 1)


def report(path):
    model = json.load(open(path))
    (xl, yl, zl), res = actual_mesh_lines(model)
    print("=====", path)
    print(" mesh resolution used by the runner: %.4f mm (%s preset)" % (res, model["settings"]["mesh"]))
    for axis, lines in zip("xyz", (xl, yl, zl)):
        st = cell_stats(lines)
        if st is None:
            print(" %s: no cells" % axis)
            continue
        print(" %s: %d lines, %d cells, min %.5f mm, max %.4f mm, worst neighbour ratio %.2fx" % (
            axis, st["n_lines"], st["n_cells"], st["min_cell"], st["max_cell"], st["worst_ratio"]))
        if st["worst_ratio"] > 2.0:
            i = int(np.argmax(np.maximum(st["cells"][1:] / st["cells"][:-1], st["cells"][:-1] / st["cells"][1:])))
            print("   >>> worst jump at %s ~ %.5f mm: cell %.5f mm next to cell %.5f mm" % (
                axis, st["line_values"][i + 1], st["cells"][i], st["cells"][i + 1]))

    # Cells across the trace width and through the prepreg, if the port geometry says what they are.
    ports_geo = runner._port_geometry(model, {"f_start": 0.5e9})  # placeholder res not needed for geometry only
    trace_w = model["ports"][0].get("track_width") if model.get("ports") else None
    if trace_w:
        for axis_name, lines in (("x", xl), ("y", yl)):
            p = model["ports"][0]
            c = p["x"] if axis_name == "y" else p["y"]  # perpendicular axis to the trace direction
        # perpendicular axis is whichever one the port's "direction" is NOT along
        p = model["ports"][0]
        perp_lines = xl if p["direction"][0] == 0 else yl
        c = p["x"] if p["direction"][0] == 0 else p["y"]
        n = cells_across(perp_lines, c - trace_w / 2, c + trace_w / 2)
        print(" cells across the first port's trace width (%.3f mm): %d" % (trace_w, n))

    layers = sorted(model["dielectric_layers"], key=lambda d: d["z_bottom"])
    for d in layers:
        n = cells_across(zl, d["z_bottom"], d["z_top"])
        print(" cells through dielectric layer z=%.4f-%.4f mm (er %.1f): %d" % (
            d["z_bottom"], d["z_top"], d["epsilon"], n))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    for p in sys.argv[1:]:
        report(p)
