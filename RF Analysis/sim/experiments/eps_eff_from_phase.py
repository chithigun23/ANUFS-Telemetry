"""Task C item 3: extract eps_eff from the phase of S21 and compare with Task A's field-solve number.

beta = -unwrap(angle(S21)) / L  (rad/m), eps_eff = (beta * c0 / (2*pi*f))^2

Run with the solver Python (C:\\openEMS\\venv\\Scripts\\python.exe):
    eps_eff_from_phase.py <results.s2p> <port1_x,y> <port2_x,y> [f_lo_hz f_hi_hz]

Port positions are in mm (as in model.json); f_lo/f_hi default to the L1/L5 GNSS bands combined
(1.0-1.7 GHz) if not given.
"""
import sys

import numpy as np
import skrf as rf

C0 = 299792458.0


def main():
    path = sys.argv[1]
    x1, y1 = (float(v) for v in sys.argv[2].split(","))
    x2, y2 = (float(v) for v in sys.argv[3].split(","))
    f_lo = float(sys.argv[4]) if len(sys.argv) > 4 else 1.0e9
    f_hi = float(sys.argv[5]) if len(sys.argv) > 5 else 1.7e9

    L = ((x2 - x1) ** 2 + (y2 - y1) ** 2) ** 0.5 / 1000.0  # mm -> m
    n = rf.Network(path)
    f = n.f
    s21 = n.s[:, 1, 0]
    phase = np.unwrap(np.angle(s21))
    beta = -phase / L  # rad/m; S21 ~ exp(-j*beta*L), phase decreases with L for a lossy/propagating line
    eps_eff = (beta * C0 / (2 * np.pi * f)) ** 2

    print("port separation L = %.4f mm" % (L * 1000.0))
    m = (f >= f_lo) & (f <= f_hi)
    if not m.any():
        sys.exit("no frequency points in the requested band")
    print("band %.3f-%.3f GHz: eps_eff = %.3f +/- %.3f (min %.3f, max %.3f)" % (
        f_lo / 1e9, f_hi / 1e9, eps_eff[m].mean(), eps_eff[m].std(), eps_eff[m].min(), eps_eff[m].max()))
    for name, lo, hi in [("L5", 1.164e9, 1.188e9), ("L1", 1.559e9, 1.606e9)]:
        mb = (f >= lo) & (f <= hi)
        if mb.any():
            print("  %s: eps_eff = %.3f" % (name, eps_eff[mb].mean()))


if __name__ == "__main__":
    if len(sys.argv) < 4:
        sys.exit(__doc__)
    main()
