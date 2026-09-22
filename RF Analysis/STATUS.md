# RF analysis status

Kept-run tracker for `Next-Steps-Sonnet.md`, which is the authoritative task brief. Updated after every task in
that file. The old `Laptop-Handover.md`, `Desktop-Handover.md` and `Short-Section-Test.md` are retired to
`archive/` and superseded by this file and the brief.

Everything not listed below (all coarse- and medium-mesh board runs, the 9.5 mm runs with ports on the SMA
clearance cutouts, and the first ANT1 run) was removed on 2026-09-22 as invalid; see the "remove invalid RF runs"
commit for the full list and confirmation that each was in git history first.

## Kept runs (`results/_keep/<machine>/...`)

| Run | Verdict | Meaning |
|---|---|---|
| `desktop/step1_short_fine_lumped` | **Valid** | Only valid board result so far: 5 mm ANT3 bare line, fine mesh (1.165 mm), lumped ports. S11 -28.8 dB (L5) / -26.4 dB (L1), S21 -0.04 / -0.05 dB, passive (max sum\|S\|^2 0.992). Port 1 only, so reciprocity unchecked. Not a Z0 measurement. |
| `laptop/step1_short_fine_lumped_long` | **Valid** | Same model as above, early stop disabled, run to 7.9 ns. Confirms the fine mesh is stable on this model (energy decays to -85 dB, no growth) and that the early-stopped result above was not contaminated (agrees to 0.0003/0.0014). |
| `laptop/validation_msl_1_6` | **Valid (toolchain check)** | Plugin's own microstrip validation board, 1-6 GHz, early stop off, stable to -107.6 dB. Evidence the solver/plugin work correctly; not board data. |
| `laptop/validation_msl_0p5_6` | **Valid (toolchain check)** | Same validation board, 0.5-6 GHz. Stable. Shows a low start frequency alone does not trigger the instability seen on board models. |
| `desktop/validation_msl_medium_0p5_3` | **Valid (toolchain check)** | Validation board at the medium mesh, 0.5-3 GHz. Stable — the plugin itself is fine at this cell size; the instability on board models is specific to our stack-up. |
| `desktop/validation_stripline_long` | **Valid (toolchain check)** | Plugin's own 4-layer stripline validation, run to the full 300,000-step cap. Stable to -97 dB, confirming the solver build is fine on a multilayer stack-up. |
| `laptop/step1_long_fixed_fine_lumped` | **Valid, but flagged** | 7 mm ANT3 section, ports moved off the SMA clearance cutouts onto plane copper, fine mesh, lumped ports, both ports excited. Passes every check in section 0 (no divergence, no power warning, passive, reciprocal), but S11 is near 0 dB and S21 is -15 to -36 dB — near-total reflection. Matches the desktop's independent finding on its ANT1 rerun. Not yet explained; candidate input to Task C. |

## Not reviewed / left alone
`desktop/step1_long_fine_ports_on_plane` — only a `model.json` committed, no results yet. Left for the desktop
session; will be reviewed once it produces a result.

## Task status (`Next-Steps-Sonnet.md`)

| Task | Status |
|---|---|
| 1. Clean up existing results | Done 2026-09-22 |
| A. Valid line impedance, no FDTD | Not started |
| B. Fix the FDTD mesh before any new board run | Not started |
| C. A validation test that can detect a Z0 error | Not started |
| D. Circuit model of each chain | Not started |
| E. One confirming 3D run | Not started |
