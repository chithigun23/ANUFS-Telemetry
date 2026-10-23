"""Recompute S-parameters from the saved port time-domain data of a finished run.

Run with the solver Python (needs CSXCAD/openEMS):
    C:\\openEMS\\venv\\Scripts\\python.exe repostprocess.py <run_dir> [excited_port_number]

<run_dir> holds model.json and exc1/ (the openEMS output of the first excitation).
No FDTD is run: this only redoes the port calculation, so it takes seconds. It prints the
result for the plugin's method (50 ohm reference impedance forced) and for openEMS's default
(the reference impedance of each line as measured), and whether each is passive.
"""
import json
import os
import sys

import numpy as np

os.add_dll_directory("C:/openEMS")
sys.path.insert(0, os.environ.get("RFSIM_PLUGIN_DIR", r"C:\openEMS\kicad-rfsim\plugins"))
import runner  # noqa: E402

run_dir = sys.argv[1]
k = int(sys.argv[2]) - 1 if len(sys.argv) > 2 else 0
model = json.load(open(os.path.join(run_dir, "model.json")))
s = model["settings"]
eps_max = max(d["epsilon"] for d in model["dielectric_layers"])
lam_min = runner.C0 / s["f_stop"] / np.sqrt(eps_max) * 1e3
res = lam_min / runner.RES_DIV[s["mesh"]]
freq = np.linspace(s["f_start"], s["f_stop"], s.get("n_freq", 401))
sim_path = os.path.join(run_dir, "exc%d" % (k + 1))
fdtd, ports, ff = runner.build(model, k, res, want_ff=False)
n = len(ports)


def report(name, S):
    band = (freq >= 1.559e9) & (freq <= 1.606e9)
    power = np.sum(np.abs(S[:, :, k]) ** 2, axis=1)
    print("%-38s S11 %6.1f..%6.1f dB  S21 %6.1f..%6.1f dB  max sum|S|^2 = %.3g  (L1 band S11 %.1f, S21 %.2f)"
          % (name,
             20 * np.log10(np.abs(S[:, 0, k]).min()), 20 * np.log10(np.abs(S[:, 0, k]).max()),
             20 * np.log10(np.abs(S[:, 1, k]).min()), 20 * np.log10(np.abs(S[:, 1, k]).max()),
             power.max(),
             20 * np.log10(np.abs(S[band, 0, k]).max()), 20 * np.log10(np.abs(S[band, 1, k]).min())))


for label, kwargs in (("plugin (ref_impedance = %g ohm)" % s["z0"], {"ref_impedance": s["z0"]}),
                      ("openEMS default (measured line Z)", {})):
    for p in ports:
        p.CalcPort(sim_path, freq, **kwargs)
    S = np.zeros((len(freq), n, n), dtype=complex)
    for j in range(n):
        S[:, j, k] = ports[j].uf_ref / ports[k].uf_inc
    report(label, S)
    for j, p in enumerate(ports):
        zi = getattr(p, "Z_ref", None)
        if zi is not None:
            zi = np.asarray(zi)
            print("    port %d Z_ref: %.1f .. %.1f ohm (median %.1f)" % (j + 1, zi.real.min(), zi.real.max(), np.median(zi.real)))
