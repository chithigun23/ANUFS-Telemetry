r"""Run a modified copy of a saved model.json, to diagnose the late-time solver instability.

Run with the solver Python (C:\openEMS\venv\Scripts\python.exe):
    run_variant.py <model.json> <out_dir> [--tand0] [--endcrit X] [--maxsteps N] [--bc PML_8|MUR] [--pec]

  --tand0     set the loss tangent of every dielectric layer to 0 (lossless)
  --endcrit   energy end criterion (default: keep the model's; 1e-12 effectively runs to the step cap)
  --maxsteps  step cap
  --excite-all    excite every port (overrides a saved model.json's settings["excite"], e.g. an old
                  single-port-only run); per Next-Steps-Sonnet.md section 3 item 5, do this from now on
  --fstart F  --fstop F   sweep limits in Hz; they set the excitation pulse and the mesh resolution
  --mesh P    mesh preset: coarse, medium or fine (much more expensive)
  --bc        boundary condition for all faces (default PML_8)
  --pec       perfect-conductor copper instead of finite-conductivity sheets
  --solid-planes  replace all copper on the layers except F.Cu by one solid rectangle covering the domain
                  (removes the real pour shapes, voids and slivers on the inner layers and B.Cu)
  --signal-only   on F.Cu keep only the polygons that cross the line between the ports (the trace and pads),
                  dropping the ground pour beside it
  --trace-width W   widen (or narrow) the F.Cu trace+pad polygons to W mm, rescaled about the line's own
                    x (a "what if this line were built wrong" test; the surrounding ground pour is left
                    where it actually is, so this is not a DRC-compliant redesign, just a sensitivity test)
  --runner-file   path to the runner script to use, relative to this file's directory
                  (default runner_var.py; ignored if --pec is also given)

The modified model and the solver output go in <out_dir>; the original is not touched.
"""
import json
import os
import subprocess
import sys

here = os.path.dirname(os.path.abspath(__file__))
args = sys.argv[1:]
src, out = args[0], os.path.abspath(args[1])
opts = args[2:]


def val(flag, cast=str):
    return cast(opts[opts.index(flag) + 1]) if flag in opts else None


m = json.load(open(src))
if "--tand0" in opts:
    for d in m["dielectric_layers"]:
        d["loss_tangent"] = 0.0
if val("--endcrit"):
    m["settings"]["end_criteria"] = val("--endcrit", float)
if val("--mesh"):
    m["settings"]["mesh"] = val("--mesh")
if val("--fstart"):
    m["settings"]["f_start"] = val("--fstart", float)
if val("--fstop"):
    m["settings"]["f_stop"] = val("--fstop", float)
if val("--maxsteps"):
    m["settings"]["max_timesteps"] = val("--maxsteps", int)
if "--excite-all" in opts:
    m["settings"].pop("excite", None)
r = m["region"]
rect = [[r["x0"], r["y0"]], [r["x1"], r["y0"]], [r["x1"], r["y1"]], [r["x0"], r["y1"]]]
if "--solid-planes" in opts:
    for name in list(m["polygons"]):
        if name != "F.Cu":
            m["polygons"][name] = [rect]
if ("--signal-only" in opts or val("--trace-width")) and len(m["ports"]) == 2:
    (xa, ya), (xb, yb) = [(p["x"], p["y"]) for p in m["ports"]]
    mid = ((xa + xb) / 2.0, (ya + yb) / 2.0)

    def keeps(poly):
        xs = [q[0] for q in poly]
        ys = [q[1] for q in poly]
        return min(xs) <= mid[0] <= max(xs) and min(ys) <= mid[1] <= max(ys) and max(xs) - min(xs) < 2.0

    if "--signal-only" in opts:
        m["polygons"]["F.Cu"] = [poly for poly in m["polygons"]["F.Cu"] if keeps(poly)]
        print("F.Cu polygons kept:", len(m["polygons"]["F.Cu"]))

    new_w = val("--trace-width", float)
    if new_w:
        line_x = xa  # both ports share the same x on a straight vertical test section
        old_w = m["ports"][0].get("track_width") or m["ports"][0]["width"]
        scale = new_w / old_w
        n_widened = 0
        for poly in m["polygons"]["F.Cu"]:
            if not keeps(poly):
                continue  # leave the ground pour where it actually is
            for pt in poly:
                pt[0] = line_x + (pt[0] - line_x) * scale
            n_widened += 1
        for p in m["ports"]:
            p["width"] = new_w
            p["length"] = new_w
            if p.get("track_width"):
                p["track_width"] = new_w
        print("trace-width: %.3f -> %.3f mm, %d F.Cu polygon(s) rescaled" % (old_w, new_w, n_widened))
os.makedirs(out, exist_ok=True)
model_path = os.path.join(out, "model.json")
json.dump(m, open(model_path, "w"), indent=1)

env = dict(os.environ)
if val("--bc"):
    env["BC"] = val("--bc")
runner = os.path.join(here, "runner_pec.py" if "--pec" in opts else (val("--runner-file") or "runner_var.py"))
with open(os.path.join(out, "solver.log"), "w") as log:
    rc = subprocess.call([sys.executable, runner, model_path, out], stdout=log, stderr=subprocess.STDOUT, env=env)
print("solver exit code", rc, "; log:", os.path.join(out, "solver.log"))
