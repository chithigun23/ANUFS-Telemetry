"""Run a modified copy of a saved model.json, to diagnose the late-time solver instability.

Run with the solver Python (C:\openEMS\venv\Scripts\python.exe):
    run_variant.py <model.json> <out_dir> [--tand0] [--endcrit X] [--maxsteps N] [--bc PML_8|MUR] [--pec]

  --tand0     set the loss tangent of every dielectric layer to 0 (lossless)
  --endcrit   energy end criterion (default: keep the model's; 1e-12 effectively runs to the step cap)
  --maxsteps  step cap
  --bc        boundary condition for all faces (default PML_8)
  --pec       perfect-conductor copper instead of finite-conductivity sheets

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
if val("--maxsteps"):
    m["settings"]["max_timesteps"] = val("--maxsteps", int)
os.makedirs(out, exist_ok=True)
model_path = os.path.join(out, "model.json")
json.dump(m, open(model_path, "w"), indent=1)

env = dict(os.environ)
if val("--bc"):
    env["BC"] = val("--bc")
runner = os.path.join(here, "runner_pec.py" if "--pec" in opts else "runner_var.py")
with open(os.path.join(out, "solver.log"), "w") as log:
    rc = subprocess.call([sys.executable, runner, model_path, out], stdout=log, stderr=subprocess.STDOUT, env=env)
print("solver exit code", rc, "; log:", os.path.join(out, "solver.log"))
