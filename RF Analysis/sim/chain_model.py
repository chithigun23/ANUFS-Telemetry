r"""Task D (Next-Steps-Sonnet.md section 5): circuit model of the ANT1 and ANT2 chains, tests H1-H4/H7.

Run with KiCad's Python (it has scikit-rf):
    "C:\Program Files\KiCad\10.0\bin\python.exe" chain_model.py [ant1|ant2|both]

No vendor Touchstone files are used (Murata SimSurfing's search UI did not cooperate with browser
automation after a genuine attempt; the user chose to proceed with ideal component models instead --
see STATUS.md, Task D). L2/L3 keep embed_l2.py's existing ideal-inductor-plus-self-resonance model
(56 nH, 2.8 GHz SRF, an assumed 1.5 ohm series resistance); the 100 pF caps and the TVS are ideal too.

Topology and lengths are derived from the real board (telemetry.kicad_pcb, read only), not assumed:
pad positions were extracted with a small S-expression parser (rotation-aware), and each physical
line section's length is the straight-line pad-to-pad distance -- an approximation (the real routed
trace has corners, so this understates true electrical length a little, most for the longest run to
the receiver pin, about 18.7 mm here). Order along the net, and which pad of each 2-pin part faces the
RF line vs. ground, was inferred from which pads sit closest together; both chains gave the same
pattern, which is some cross-check that the inference is right; treat exact internal bias-network
wiring (where precisely R39/R42 sits relative to C30/C31 and C51/C52) as a simplification, not as
read off the schematic -- flagged in STATUS.md.

Topology, SMA to receiver:
    J7/J8 (connector, treated as an ideal 50 ohm launch -- Step 2 is out of scope here)
      -- line --
    D4/D5 TVS: shunt cap (0.05 pF, per RF-Analysis-Plan.md) + a 0.3 pF pad cap + a 0.5 nH via to ground
      -- line --
    L2/L3 T-junction: shunt branch = ideal 56 nH inductor (2.8 GHz self-resonant cap, 1.5 ohm series,
        embed_l2.py's model) -- R39/R42 (10 ohm) -- shunt (C30||C31, or C51||C52) to ground, each with
        a 0.2 pF pad cap and a 0.5 nH via
      -- line --
    C27/C48: series 100 pF DC block, 0.2 pF pad cap on each pad
      -- line --
    C28/C49 (empty pad, pi-filter shunt 1): 0.2 pF pad cap only, shunt to ground through a 0.5 nH via
      -- line --
    R35/R42 (0 ohm, pi-filter series link) -- modelled as a short, no parasitic beyond its own pads
      -- line --
    C29/C50 (empty pad, pi-filter shunt 2): same as C28/C49
      -- line --
    U10/U13 RF_IN pin (100 ohm differential pin modelled single-ended at 50 ohm; out of scope to redo
        the receiver's own input impedance here -- port 2 is left at the system Z0)

Sensitivity sweep: each parasitic is swept 0.5x-3x its assumed value, one at a time, and the run
reports the multiplier (if any, within that range) where a RF-Analysis-Plan.md section 7 criterion
first fails in the L1/L5 bands.
"""
import argparse
import itertools
import json
import math
import os
import sys

import numpy as np
import skrf as rf

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
FREQ = rf.Frequency(0.5, 3.0, 251, unit="ghz")
Z0_SYS = 50.0
C0 = 299792458.0

# Plan section 7 acceptance criteria (this script checks the ones a 2-port S11/S21 chain model can:
# not RF-to-bias isolation (H8, needs port 4) or the connector launch (H6, needs J7/J8 in the model)).
BANDS = {"L5": (1.164e9, 1.188e9), "L1": (1.559e9, 1.606e9)}
CRITERIA = {
    "S11_dB_max": -10.0,       # S11 looking into the SMA: -10 dB or better
    "S21_loss_dB_max": 0.5,    # insertion loss: 0.5 dB or better (i.e. S21 >= -0.5 dB)
    "S21_ripple_dB_max": 0.3,  # in-band ripple in S21: under 0.3 dB
}

# Task A field-solve Z0/eps_eff (results/_keep and STATUS.md "Task A" section). ANT3 has no chain, not used here.
TASK_A = {
    "ANT1": {"z0": 55.47, "eps_eff": 3.229},
    "ANT2": {"z0": 55.71, "eps_eff": 3.241},
}

# Geometry-derived segment lengths in mm (see module docstring), and the parasitic values to sweep.
CHAINS = {
    "ANT1": {
        "segments_mm": [5.14, 2.43, 0.847, 1.121, 0.5, 0.5, 18.72],
        # J7-D4, D4-L2junction, L2junction-C27, C27-R35(via C28 node), C28-R35, R35-C29, C29-U10
        "bias_L": "L2", "bias_C_pF": (100.0, 1.0e5), "bias_R_ohm": 10.0,  # C30 100pF, C31 100nF (1e5 pF)
        "dc_block_pF": 100.0,  # C27
    },
    "ANT2": {
        "segments_mm": [4.91, 1.861, 0.847, 1.121, 0.5, 0.5, 18.67],
        "bias_L": "L3", "bias_C_pF": (100.0, 1.0e5),  # C51, C52
        "bias_R_ohm": 10.0,  # R42
        "dc_block_pF": 100.0,  # C48
    },
}

PAD_CAP_PF = 0.2       # every populated 0402 pad
TVS_PAD_CAP_PF = 0.3   # the TVS's own pad, per the brief
VIA_L_NH = 0.5         # ground-via inductance
TVS_C_PF = 0.05        # D4/D5, per RF-Analysis-Plan.md
L2_NH = 56.0
L2_SRF_GHZ = 2.8
L2_R_OHM = 1.5


def line(media, length_mm):
    return media.line(length_mm, unit="mm")


def cap_via_short_1port(media, pf, via_nh=VIA_L_NH):
    """A capacitor (optionally in series with a ground-via inductance) terminated in a hard short --
    a 1-port network: "looking into this branch from the line, toward ground"."""
    if via_nh:
        return media.capacitor(pf * 1e-12) ** media.inductor(via_nh * 1e-9) ** media.short()
    return media.capacitor(pf * 1e-12) ** media.short()


def shunt_cap(media, pf):
    """A capacitor from the line to ground, as a 2-port element ready to cascade into a chain."""
    return media.shunt(cap_via_short_1port(media, pf, via_nh=None))


def shunt_lc_via(media, pf, via_nh=VIA_L_NH):
    """A shunt element (a pad, or a decoupling cap) with its own parasitic pad cap already added by
    the caller, in series with a ground-via inductance -- as a 2-port ready to cascade into a chain."""
    return media.shunt(cap_via_short_1port(media, pf, via_nh))


def inductor_with_srf(media, l_nh, srf_ghz, r_ohm):
    """L2/L3: an ideal inductor with a parallel self-resonant capacitance and a series resistance,
    the same model embed_l2.py uses (assumed values, not a vendor file -- see module docstring)."""
    l = l_nh * 1e-9
    c_par = 1.0 / ((2 * math.pi * srf_ghz * 1e9) ** 2 * l)
    w = 2 * math.pi * FREQ.f
    y_l = 1.0 / (1j * w * l) + 1j * w * c_par
    z_l = 1.0 / y_l + r_ohm
    s = np.zeros((len(FREQ), 2, 2), dtype=complex)
    s11 = z_l / (z_l + 2 * Z0_SYS)
    s21 = 2 * Z0_SYS / (z_l + 2 * Z0_SYS)
    s[:, 0, 0] = s[:, 1, 1] = s11
    s[:, 0, 1] = s[:, 1, 0] = s21
    return rf.Network(frequency=FREQ, s=s, z0=Z0_SYS)


def build_chain(name, params, pad_cap_pf=PAD_CAP_PF, tvs_pad_cap_pf=TVS_PAD_CAP_PF, via_nh=VIA_L_NH,
                 tvs_c_pf=TVS_C_PF, l_nh=L2_NH, srf_ghz=L2_SRF_GHZ, l_r_ohm=L2_R_OHM,
                 dc_block_pf=None, bias_r_ohm=None, bias_c_scale=1.0):
    z0 = TASK_A[name]["z0"]
    eps_eff = TASK_A[name]["eps_eff"]
    gamma = 1j * 2 * math.pi * FREQ.f * math.sqrt(eps_eff) / C0
    media = rf.media.DefinedGammaZ0(frequency=FREQ, gamma=gamma, z0=z0)
    segs = params["segments_mm"]
    dc_block_pf = params["dc_block_pF"] if dc_block_pf is None else dc_block_pf
    bias_r_ohm = params["bias_R_ohm"] if bias_r_ohm is None else bias_r_ohm
    c30_pf, c31_pf = params["bias_C_pF"]
    c30_pf, c31_pf = c30_pf * bias_c_scale, c31_pf * bias_c_scale

    net = media.thru()
    net = net ** line(media, segs[0])
    # D4/D5 TVS: shunt cap + its own pad cap, via a ground via
    net = net ** shunt_lc_via(media, tvs_c_pf + tvs_pad_cap_pf, via_nh)
    net = net ** line(media, segs[1])
    # L2/L3 T-junction: shunt branch = inductor -> R39/R42 -> (C30||C31) to ground, all in series
    # down the branch, ending in a hard short -- a single 1-port, then tapped onto the line once.
    # C30 and C31 both shunt to ground at the bias node; modelled as their combined parallel value.
    l_net = inductor_with_srf(media, l_nh, srf_ghz, l_r_ohm)
    branch_1port = l_net ** media.resistor(bias_r_ohm) ** cap_via_short_1port(
        media, c30_pf + c31_pf + 2 * pad_cap_pf, via_nh)
    net = net ** media.shunt(branch_1port)
    net = net ** line(media, segs[2])
    # C27/C48 series DC block (its own pad parasitics are second-order next to a 100 pF series
    # element at these frequencies and are not modelled separately here)
    net = net ** media.capacitor(dc_block_pf * 1e-12)
    net = net ** line(media, segs[3])
    # C28/C49: empty pad, shunt stray cap through a via
    net = net ** shunt_lc_via(media, pad_cap_pf, via_nh)
    net = net ** line(media, segs[4])
    # R35/R42: 0 ohm series link (a short)
    net = net ** media.resistor(1e-6)
    net = net ** line(media, segs[5])
    # C29/C50: empty pad, shunt stray cap through a via
    net = net ** shunt_lc_via(media, pad_cap_pf, via_nh)
    net = net ** line(media, segs[6])
    return net


def band_metrics(net, lo, hi):
    f = net.f
    m = (f >= lo) & (f <= hi)
    s11_db = 20 * np.log10(np.maximum(np.abs(net.s[m, 0, 0]), 1e-12))
    s21_db = 20 * np.log10(np.maximum(np.abs(net.s[m, 1, 0]), 1e-12))
    return {
        "S11_dB_max": float(s11_db.max()),
        "S21_loss_dB_max": float(-s21_db.min()),
        "S21_ripple_dB_max": float(s21_db.max() - s21_db.min()),
    }


def report_bands(net, label):
    print("%s:" % label)
    for band, (lo, hi) in BANDS.items():
        m = band_metrics(net, lo, hi)
        flags = []
        if m["S11_dB_max"] > CRITERIA["S11_dB_max"]:
            flags.append("S11 FAILS")
        if m["S21_loss_dB_max"] > CRITERIA["S21_loss_dB_max"]:
            flags.append("insertion loss FAILS")
        if m["S21_ripple_dB_max"] > CRITERIA["S21_ripple_dB_max"]:
            flags.append("ripple FAILS")
        print("  %s: S11 worst %.2f dB, insertion loss worst %.3f dB, ripple %.3f dB %s" % (
            band, m["S11_dB_max"], m["S21_loss_dB_max"], m["S21_ripple_dB_max"],
            ("[" + ", ".join(flags) + "]") if flags else "[pass]"))
    return m


SENSITIVITY_PARAMS = {
    "pad_cap_pf": (PAD_CAP_PF, "pad_cap_pf"),
    "via_nh": (VIA_L_NH, "via_nh"),
    "tvs_c_pf": (TVS_C_PF, "tvs_c_pf"),
    "l_r_ohm": (L2_R_OHM, "l_r_ohm"),
    "bias_c_scale": (1.0, "bias_c_scale"),
}


def worst_case_metric(net):
    worst = {"S11_dB_max": -999.0, "S21_loss_dB_max": -999.0, "S21_ripple_dB_max": -999.0}
    for lo, hi in BANDS.values():
        m = band_metrics(net, lo, hi)
        for k in worst:
            worst[k] = max(worst[k], m[k])
    return worst


def sensitivity_sweep(name, params):
    print("\nSensitivity sweep, %s (0.5x-3x each parasitic, one at a time):" % name)
    base = worst_case_metric(build_chain(name, params))
    for param, (default, kwarg) in SENSITIVITY_PARAMS.items():
        failing_at = {}
        for mult in [0.5, 0.75, 1.0, 1.25, 1.5, 2.0, 2.5, 3.0]:
            kwargs = {kwarg: default * mult}
            net = build_chain(name, params, **kwargs)
            m = worst_case_metric(net)
            for crit, limit in CRITERIA.items():
                if m[crit] > limit and crit not in failing_at:
                    failing_at[crit] = mult
        base_ok = all(base[c] <= limit for c, limit in CRITERIA.items())
        print("  %-14s base=%.3g  " % (param, default) + (
            ", ".join("%s fails at %sx" % (c, m) for c, m in failing_at.items())
            if failing_at else "no criterion fails within 0.5x-3x"
        ) + ("" if base_ok else "  [ALREADY FAILING AT 1x]"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("chain", nargs="?", default="both", choices=["ant1", "ant2", "both"])
    args = ap.parse_args()
    names = ["ANT1", "ANT2"] if args.chain == "both" else [args.chain.upper()]

    outdir = os.path.join(REPO, "RF Analysis", "results", "chain_model")
    os.makedirs(outdir, exist_ok=True)
    summary = {}
    for name in names:
        params = CHAINS[name]
        net = build_chain(name, params)
        print("=" * 70)
        print(name, "chain: Z0=%.2f ohm, eps_eff=%.3f (Task A), total line length %.1f mm" % (
            TASK_A[name]["z0"], TASK_A[name]["eps_eff"], sum(params["segments_mm"])))
        m = report_bands(net, name)
        net.write_touchstone(os.path.join(outdir, "%s_chain" % name.lower()), form="ri")
        sensitivity_sweep(name, params)
        summary[name] = m
    print("\n" + "=" * 70)
    print("Hypotheses this model can speak to (H1-H4, H7; H6/H8 need the connector/port-4 model):")
    for name in names:
        m = summary[name]
        ok = all(m[c] <= limit for c, limit in CRITERIA.items())
        print("  %s: %s with the ideal-component model -- %s" % (
            name, "passes comfortably" if ok else "does NOT pass",
            "still needs 3D confirmation (Task E)" if ok else "needs investigation before Task E"))


if __name__ == "__main__":
    main()
