"""Print S-parameter figures for the GNSS bands from a Touchstone results file.

Run with the solver Python:  C:\openEMS\venv\Scripts\python.exe bands.py <results.s2p>
Two-port files only. Prints worst-case S11 and S21 in L5 and L1, and the passivity check
(|S11|^2 + |S21|^2 must not exceed 1 for a passive structure, up to solver accuracy).
"""
import sys

import numpy as np

rows = np.loadtxt(sys.argv[1], comments=("!", "#"))
f = rows[:, 0]
s11 = np.abs(rows[:, 1] + 1j * rows[:, 2])
s21 = np.abs(rows[:, 3] + 1j * rows[:, 4])
db = lambda x: 20 * np.log10(np.maximum(x, 1e-12))  # noqa: E731
power = s11 ** 2 + s21 ** 2
for name, (a, b) in {"L5": (1.164e9, 1.188e9), "L1": (1.559e9, 1.606e9)}.items():
    m = (f >= a) & (f <= b)
    print("%s  S11 worst %6.1f dB   S21 worst %7.3f dB   max |S11|^2+|S21|^2 = %.3f"
          % (name, db(s11[m]).max(), db(s21[m]).min(), power[m].max()))
print("whole sweep  S11 worst %.1f dB   S21 worst %.3f dB   max power sum %.3f"
      % (db(s11).max(), db(s21).min(), power.max()))
