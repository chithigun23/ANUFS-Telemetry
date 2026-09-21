"""Circuit stage for the ANT1 4-port extraction: put the inductor L2 back between ports 3 and 4.

Run with KiCad's Python (it has scikit-rf):
    "C:\\Program Files\\KiCad\\10.0\\bin\\python.exe" embed_l2.py <results.s4p> [out_prefix]

The 3D model has L2 removed and a port on each of its two pads (port 3 on the RF-line side, port 4 on the
bias-network side). This script connects a model of L2 between those two ports, which leaves the 2-port
chain (port 1 = after the SMA, port 2 = the receiver RF_IN pad), and prints S11 and S21 in the GNSS bands.
It also writes <out_prefix>_chain.s2p and, for comparison, <out_prefix>_noL2.s2p with L2 replaced by an open
circuit (ports 3 and 4 left open, as if the bias tee were not fitted).

L2 model (Murata LQW15AN56NG00D, 56 nH): ideal inductor in parallel with a small capacitance that puts the
self-resonance at 2.8 GHz (the datasheet SRF), in series with a resistance. The resistance is an ASSUMED 1.5 ohm,
a placeholder for the datasheet DC/RF resistance. Replace this with the manufacturer's S-parameter file when it
is available (see RF-Analysis-Plan.md, section 5).

Not in the 3D model, so not in this result: D4 (TVS, about 0.05 pF from the RF line to ground), the rest of the
supply network beyond R39, and the connector launch (J7).
"""
import sys

import numpy as np
import skrf as rf

src = sys.argv[1]
prefix = sys.argv[2] if len(sys.argv) > 2 else src.rsplit(".", 1)[0]
net = rf.Network(src)
f = net.frequency
w = 2 * np.pi * f.f
z0 = 50.0
L, srf, r_series = 56e-9, 2.8e9, 1.5
c_par = 1.0 / ((2 * np.pi * srf) ** 2 * L)
y_l = 1.0 / (1j * w * L) + 1j * w * c_par           # inductor with its parallel capacitance
z_l = 1.0 / y_l + r_series                           # plus series resistance
# 2-port S of a series impedance Z between two z0 ports
s11 = z_l / (z_l + 2 * z0)
s21 = 2 * z0 / (z_l + 2 * z0)
s = np.zeros((len(f), 2, 2), dtype=complex)
s[:, 0, 0] = s[:, 1, 1] = s11
s[:, 0, 1] = s[:, 1, 0] = s21
l2 = rf.Network(frequency=f, s=s, z0=z0, name="L2")

# connect L2 port 0 to network port 2 (index), then the other L2 port to network port 3
joined = rf.network.connect(net, 2, l2, 0)          # the 4-port with port 2 (index) replaced by L2's far port, now the last port
# after connect, ports: net ports 0,1,3 keep their order, then L2's remaining port is appended last
joined = rf.network.innerconnect(joined, 2, 3)      # indices: net port 3 became index 2; L2 far port is index 3
chain = joined
chain.name = "chain"
chain.write_touchstone(prefix + "_chain", form="ri")


def band(n, name, lo, hi):
    m = (n.f >= lo) & (n.f <= hi)
    s11d = 20 * np.log10(np.abs(n.s[m, 0, 0]))
    s21d = 20 * np.log10(np.abs(n.s[m, 1, 0]))
    p = np.abs(n.s[m, 0, 0]) ** 2 + np.abs(n.s[m, 1, 0]) ** 2
    print("  %s  S11 worst %6.1f dB   S21 worst %7.3f dB   |S11|^2+|S21|^2 max %.3f" % (name, s11d.max(), s21d.min(), p.max()))


print("Chain with L2 fitted (port 1 -> port 2):")
band(chain, "L5", 1.164e9, 1.188e9)
band(chain, "L1", 1.559e9, 1.606e9)

# reference: L2 open (ports 3 and 4 open circuits, no bias tee), from the same 4-port data
s4 = net.s
# terminate ports 3 and 4 with open circuits (reflection coefficient +1) and reduce to a 2-port
s2 = np.zeros((len(f), 2, 2), dtype=complex)
for k in range(len(f)):
    a = s4[k]
    idx_p, idx_t = [0, 1], [2, 3]
    gt = np.eye(2)
    m = np.linalg.solve(np.eye(2) - a[np.ix_(idx_t, idx_t)] @ gt, a[np.ix_(idx_t, idx_p)])
    s2[k] = a[np.ix_(idx_p, idx_p)] + a[np.ix_(idx_p, idx_t)] @ gt @ m
noL2 = rf.Network(frequency=f, s=s2, z0=z0, name="noL2")
noL2.write_touchstone(prefix + "_noL2", form="ri")
print("Same layout with the bias tee open (no L2, ports 3 and 4 open):")
band(noL2, "L5", 1.164e9, 1.188e9)
band(noL2, "L1", 1.559e9, 1.606e9)
