"""Task A (Next-Steps-Sonnet.md section 2): valid line impedance, no FDTD. Closes hypothesis H5.

Reads telemetry.kicad_pcb as plain text (read only, no pcbnew, no modification) and:
 1. Parses the ANT1/ANT2/ANT3 F.Cu trace segments and the "Ground Pour" GND zone's cached filled-polygon
    islands on F.Cu (the coplanar pour) and In1.Cu (the reference plane 0.2104 mm below F.Cu).
 2. Measures the actual as-fabricated coplanar gap (trace edge to pour edge) at sample points along each line,
    and confirms In1.Cu coverage under the trace.
 3. Computes Z0 and eps_eff two independent ways: (a) closed-form grounded-CPW (GCPW) formulas, and
    (b) a 2D finite-difference Laplace solve (energy method) of the same cross-section. Also reports the plain
    microstrip value (no coplanar gap, i.e. gap -> infinity) for comparison.

Run: "C:\\Program Files\\KiCad\\10.0\\bin\\python.exe" task_a_line_impedance.py
"""
import os
import re

import numpy as np
from scipy.sparse import lil_matrix
from scipy.sparse.linalg import spsolve
from scipy.special import ellipk

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
BOARD = os.path.join(REPO, "telemetry-kicad", "telemetry.kicad_pcb")

# Stack-up (from board_reader's extracted model.json, F.Cu to In1.Cu): 35 um copper, 0.2104 mm prepreg, er 4.4.
H_MM = 0.2104
ER = 4.4
COPPER_UM = 35
Z0_TARGET = 50.0
LINES = ("ANT1", "ANT2", "ANT3")


# ---------- minimal S-expression parser (the .kicad_pcb format is plain balanced parens) ----------

def parse_sexpr(text):
    tokens = re.findall(r'\(|\)|"[^"]*"|[^\s()]+', text)
    pos = 0

    def parse():
        nonlocal pos
        node = []
        while pos < len(tokens):
            t = tokens[pos]
            pos += 1
            if t == "(":
                node.append(parse())
            elif t == ")":
                return node
            else:
                node.append(t.strip('"'))
        return node

    return parse()[0]


def walk(node, tag, out):
    if isinstance(node, list) and node and node[0] == tag:
        out.append(node)
    if isinstance(node, list):
        for c in node:
            walk(c, tag, out)


def field(node, tag):
    for c in node[1:]:
        if isinstance(c, list) and c and c[0] == tag:
            return c
    return None


def xy_list(pts_node):
    return [(float(p[1]), float(p[2])) for p in pts_node[1:] if isinstance(p, list) and p[0] == "xy"]


# ---------- geometry ----------

def point_in_poly(pt, ring):
    x, y = pt
    hit = False
    n = len(ring)
    for i in range(n):
        x1, y1 = ring[i]
        x2, y2 = ring[(i + 1) % n]
        if (y1 > y) != (y2 > y) and x < (x2 - x1) * (y - y1) / (y2 - y1) + x1:
            hit = not hit
    return hit


def covered(pt, islands):
    return any(point_in_poly(pt, ring) for ring in islands)


def clip_ring(ring, x0, y0, x1, y1):
    """Sutherland-Hodgman clip of a (possibly huge) closed ring to an axis-aligned box. Keeps the
    ring's topology valid inside the box, so a point-in-polygon test against the clipped ring gives
    the same answer as against the original ring for any point inside the box, but with far fewer
    vertices to test against. One board-wide polygon (~25,000 vertices) makes the naive O(n) test
    too slow to run at the sample counts this script needs."""
    def clip_edge(poly, keep, intersect):
        out = []
        n = len(poly)
        for i in range(n):
            cur, nxt = poly[i], poly[(i + 1) % n]
            cur_in, nxt_in = keep(cur), keep(nxt)
            if cur_in:
                out.append(cur)
            if cur_in != nxt_in:
                out.append(intersect(cur, nxt))
        return out

    def ix_x(x0):
        return lambda a, b: (x0, a[1] + (b[1] - a[1]) * (x0 - a[0]) / (b[0] - a[0]))

    def ix_y(y0):
        return lambda a, b: (a[0] + (b[0] - a[0]) * (y0 - a[1]) / (b[1] - a[1]), y0)

    poly = list(ring)
    for keep, intersect in [
        (lambda p: p[0] >= x0, ix_x(x0)),
        (lambda p: p[0] <= x1, ix_x(x1)),
        (lambda p: p[1] >= y0, ix_y(y0)),
        (lambda p: p[1] <= y1, ix_y(y1)),
    ]:
        if not poly:
            return []
        poly = clip_edge(poly, keep, intersect)
    return poly


def clip_islands(islands, x0, y0, x1, y1, margin_mm=0.5):
    x0, y0, x1, y1 = x0 - margin_mm, y0 - margin_mm, x1 + margin_mm, y1 + margin_mm
    out = []
    for ring in islands:
        xs = [p[0] for p in ring]
        ys = [p[1] for p in ring]
        if max(xs) < x0 or min(xs) > x1 or max(ys) < y0 or min(ys) > y1:
            continue  # island's bounding box does not reach the window at all
        clipped = clip_ring(ring, x0, y0, x1, y1)
        if clipped:
            out.append(clipped)
    return out


def load_board():
    with open(BOARD, encoding="utf-8") as fh:
        tree = parse_sexpr(fh.read())

    segments = []
    walk(tree, "segment", segments)
    ant_segs = {name: [] for name in LINES}
    for s in segments:
        net = field(s, "net")
        layer = field(s, "layer")
        if net and layer and net[1] in ant_segs and layer[1] == "F.Cu":
            start = field(s, "start")
            end = field(s, "end")
            width = field(s, "width")
            ant_segs[net[1]].append({
                "a": (float(start[1]), float(start[2])),
                "b": (float(end[1]), float(end[2])),
                "width": float(width[1]),
            })

    zones = []
    walk(tree, "zone", zones)
    pour = None
    for z in zones:
        net = field(z, "net")
        name = field(z, "name")
        if net and net[1] == "GND" and name and name[1] == "Ground Pour":
            pour = z
            break
    if pour is None:
        raise SystemExit("Ground Pour zone (net GND) not found")

    fcu_islands, in1_islands = [], []
    for c in pour[1:]:
        if isinstance(c, list) and c[0] == "filled_polygon":
            layer = field(c, "layer")
            pts = field(c, "pts")
            if layer[1] == "F.Cu":
                fcu_islands.append(xy_list(pts))
            elif layer[1] == "In1.Cu":
                in1_islands.append(xy_list(pts))
    return ant_segs, fcu_islands, in1_islands


def measure_gaps(ant_segs, fcu_islands, in1_islands, step_mm=0.25, probe_max_mm=3.0, probe_res_mm=0.005):
    print("Line  segments  samples  gap_min_mm  gap_max_mm  gap_median_mm  In1.Cu_under_trace")
    results = {}
    for name, segs in ant_segs.items():
        if not segs:
            print("%-4s  (no F.Cu segments found)" % name)
            continue
        xs = [p[0] for seg in segs for p in (seg["a"], seg["b"])]
        ys = [p[1] for seg in segs for p in (seg["a"], seg["b"])]
        window = (min(xs), min(ys), max(xs), max(ys))
        local_fcu = clip_islands(fcu_islands, *window, margin_mm=probe_max_mm + 0.5)
        local_in1 = clip_islands(in1_islands, *window, margin_mm=0.5)
        gaps = []
        in1_ok = []
        for seg in segs:
            ax, ay = seg["a"]
            bx, by = seg["b"]
            length = ((bx - ax) ** 2 + (by - ay) ** 2) ** 0.5
            if length < 1e-9:
                continue
            ux, uy = (bx - ax) / length, (by - ay) / length
            nx, ny = -uy, ux  # unit perpendicular
            half_w = seg["width"] / 2.0
            n_samples = max(2, int(length / step_mm))
            for i in range(n_samples + 1):
                t = i / n_samples
                cx, cy = ax + t * (bx - ax), ay + t * (by - ay)
                in1_ok.append(covered((cx, cy), local_in1))
                for sign in (1.0, -1.0):
                    edge = (cx + sign * nx * half_w, cy + sign * ny * half_w)
                    # march outward from the trace edge until the point falls inside the pour
                    d = 0.0
                    found = None
                    while d <= probe_max_mm:
                        p = (edge[0] + sign * nx * d, edge[1] + sign * ny * d)
                        if covered(p, local_fcu):
                            found = d
                            break
                        d += probe_res_mm
                    if found is not None:
                        gaps.append(found)
        if gaps:
            g = np.array(gaps)
            print("%-4s  %-9d %-8d %-11.3f %-11.3f %-14.3f %s" % (
                name, len(segs), len(gaps), g.min(), g.max(), np.median(g),
                "all points covered" if all(in1_ok) else "%d/%d NOT covered" % (len(in1_ok) - sum(in1_ok), len(in1_ok))))
            results[name] = {"gap_mm": float(np.median(g)), "gap_min_mm": float(g.min()), "gap_max_mm": float(g.max())}
        else:
            print("%-4s  %-9d 0 samples -- gap not found within %.1f mm" % (name, len(segs), probe_max_mm))
    return results


# ---------- (a) closed-form grounded CPW (Ghione & Naldi 1984 / Wadell): sinh scaling for a single
# backside ground plane (as opposed to tanh, used for two-sided/shielded backing). ----------

def K(k):
    return ellipk(k * k)  # scipy uses the parameter m = k^2, not the modulus k


def gcpw_closed_form(w_mm, s_mm, h_mm, er):
    a = w_mm / 2.0
    b = a + s_mm
    k = a / b
    kp = (1 - k * k) ** 0.5
    k3 = np.sinh(np.pi * a / (4 * h_mm)) / np.sinh(np.pi * b / (4 * h_mm))
    k3p = (1 - k3 * k3) ** 0.5
    ratio_air = K(kp) / K(k)
    ratio_gnd = K(k3) / K(k3p)
    eps_eff = (1 + er * ratio_gnd / ratio_air) / (1 + ratio_gnd / ratio_air)
    z0 = (60 * np.pi) / (eps_eff ** 0.5) / (1 / ratio_air + ratio_gnd)
    return z0, eps_eff


def microstrip_closed_form(w_mm, h_mm, er):
    # Hammerstad-Jensen, wide-trace (w/h > 1) case; used only as a plain-microstrip comparison point.
    u = w_mm / h_mm
    eps_eff = (er + 1) / 2 + (er - 1) / 2 * (1 + 12 / u) ** -0.5
    z0 = (120 * np.pi) / (eps_eff ** 0.5) / (u + 1.393 + 0.667 * np.log(u + 1.444))
    return z0, eps_eff


# ---------- (b) 2D finite-difference Laplace solve (energy method) of the same cross-section ----------

def fd_gcpw(w_mm, s_mm, h_mm, er, pad_x_mm=None, pad_z_mm=None, res_mm=0.006):
    # Grid sizing note: nx*nz unknowns go into a direct sparse solve (spsolve). Keep the domain to a
    # handful of h/s beyond the conductors and the resolution no finer than needed to place a few
    # cells across the gap, or the unknown count runs into the millions and spsolve stalls.
    a = w_mm / 2000.0  # to metres
    b = a + s_mm / 1000.0
    h = h_mm / 1000.0
    res_mm = min(res_mm, s_mm / 6.0, h_mm / 6.0)  # a few cells across the gap and the substrate height
    res = res_mm / 1000.0
    pad_x = (pad_x_mm or 8 * s_mm) / 1000.0
    pad_z = (pad_z_mm or 8 * h_mm) / 1000.0
    xmax = b + pad_x
    zmax = h + pad_z

    nx = int(2 * xmax / res) + 1
    nz = int(zmax / res) + 1
    xs = np.linspace(-xmax, xmax, nx)
    zs = np.linspace(0.0, zmax, nz)
    dx = xs[1] - xs[0]
    dz = zs[1] - zs[0]
    iz_h = int(round(h / dz))  # grid row of the F.Cu / trace plane

    def eps_at(iz):
        # eps just BELOW row iz (between iz-1 and iz): er under the trace plane, 1 (air) above it.
        return er if iz <= iz_h else 1.0

    def is_center(ix):
        return abs(xs[ix]) <= a + 1e-12

    def is_ground_strip(ix):
        return abs(xs[ix]) >= b - 1e-12

    def solve(er_use):
        N = nx * nz
        A = lil_matrix((N, N))
        rhs = np.zeros(N)

        def idx(ix, iz):
            return iz * nx + ix

        for iz in range(nz):
            for ix in range(nx):
                k = idx(ix, iz)
                # Dirichlet: bottom ground plane, domain sides, top far boundary, and the two conductors
                if iz == 0 or iz == nz - 1 or ix == 0 or ix == nx - 1:
                    A[k, k] = 1.0
                    rhs[k] = 0.0
                    continue
                if iz == iz_h and is_center(ix):
                    A[k, k] = 1.0
                    rhs[k] = 1.0
                    continue
                if iz == iz_h and is_ground_strip(ix):
                    A[k, k] = 1.0
                    rhs[k] = 0.0
                    continue
                # interior node: flux-conserving 5-point stencil. eps is er below the F.Cu plane (row
                # iz_h) and 1 (air) above it; the flux link between two rows uses the material that
                # actually occupies that link, so the link straddling iz_h correctly sees the boundary.
                eps_here_up = er_use if (iz + 1) <= iz_h else 1.0
                eps_here_dn = er_use if iz <= iz_h else 1.0
                eps_lr = er_use if iz <= iz_h else 1.0
                cN = eps_here_up / dz ** 2
                cS = eps_here_dn / dz ** 2
                cE = eps_lr / dx ** 2
                cW = eps_lr / dx ** 2
                A[k, k] = -(cN + cS + cE + cW)
                A[k, idx(ix, iz + 1)] = cN
                A[k, idx(ix, iz - 1)] = cS
                A[k, idx(ix + 1, iz)] = cE
                A[k, idx(ix - 1, iz)] = cW
        V = spsolve(A.tocsr(), rhs).reshape(nz, nx)
        # energy method: W = 0.5 * sum(eps * |grad V|^2) dV  (per unit length, 2D), C = 2W (V=1 V applied)
        Ex = (V[:, 1:] - V[:, :-1]) / dx
        Ez = (V[1:, :] - V[:-1, :]) / dz
        # eps for each Ex cell (horizontal link on row iz) and Ez cell (vertical link between iz, iz+1):
        eps_row = np.where(np.arange(nz) <= iz_h, er_use, 1.0)
        eps_ex = np.repeat(eps_row[:, None], nx - 1, axis=1)
        eps_ez = np.repeat(((eps_row[:-1] + eps_row[1:]) / 2.0)[:, None], nx, axis=1)
        wx = 0.5 * np.sum(eps_ex * Ex ** 2) * dx * dz
        wz = 0.5 * np.sum(eps_ez * Ez ** 2) * dx * dz
        W = wx + wz
        EPS0 = 8.8541878128e-12
        C = 2 * W * EPS0
        return C

    c_diel = solve(er)
    c_air = solve(1.0)
    eps_eff = c_diel / c_air
    c_light = 299792458.0
    z0 = 1.0 / (c_light * (c_diel * c_air) ** 0.5)
    return z0, eps_eff, dict(nx=nx, nz=nz, dx_um=dx * 1e6, dz_um=dz * 1e6)


def main():
    ant_segs, fcu_islands, in1_islands = load_board()
    print("Ground Pour: %d F.Cu islands, %d In1.Cu islands\n" % (len(fcu_islands), len(in1_islands)))
    gaps = measure_gaps(ant_segs, fcu_islands, in1_islands)

    trace_w = None
    for segs in ant_segs.values():
        if segs:
            trace_w = segs[0]["width"]
            break
    print("\nTrace width used: %.3f mm; h = %.4f mm; er = %.1f; copper %d um\n" % (trace_w, H_MM, ER, COPPER_UM))

    print("Line   gap_mm   Z0_closedform_ohm  epsEff_cf   Z0_fdsolve_ohm  epsEff_fd   agreement")
    for name in LINES:
        if name not in gaps:
            continue
        g = gaps[name]["gap_mm"]
        z0_cf, e_cf = gcpw_closed_form(trace_w, g, H_MM, ER)
        z0_fd, e_fd, grid = fd_gcpw(trace_w, g, H_MM, ER)
        agree = abs(z0_cf - z0_fd) / z0_fd * 100
        print("%-5s  %-8.3f %-18.2f %-11.3f %-15.2f %-11.3f %.1f%% (grid %dx%d, %.0f um)" % (
            name, g, z0_cf, e_cf, z0_fd, e_fd, agree, grid["nx"], grid["nz"], grid["dx_um"]))

    z0_ms, e_ms = microstrip_closed_form(trace_w, H_MM, ER)
    print("\nPlain microstrip (no coplanar gap), for comparison: Z0 = %.2f ohm, eps_eff = %.3f" % (z0_ms, e_ms))

    print("\nTarget: %.0f ohm +/- 5%% (%.1f - %.1f ohm)" % (Z0_TARGET, Z0_TARGET * 0.95, Z0_TARGET * 1.05))


if __name__ == "__main__":
    main()
