"""Check that every port of a saved model.json has reference-plane copper under it.

Run with any Python (standard library only):
    python port_coverage.py <model.json> [more model.json ...]

Why: a lumped port drives a voltage between its signal pad and the copper of its reference layer. If there is no
copper on the reference layer at the port position (for example the clearance cutout under an SMA launch), the
port has no return path and the run gives total reflection (S11 near 0 dB, S21 near -60 dB) or nonsense. The 9.5 mm
ANT3 section had both ports on such cutouts. Run this on a model BEFORE spending solver time on it.

For each port it prints whether the signal layer and the reference layer have copper at the port, and for a
two-port line it prints a coverage strip along the line (# = copper, . = none) for the reference layer.

Method: even-odd point-in-polygon over every ring on the layer, which is correct when holes are cut into the
outline as keyholes or given as separate rings. If a model stores overlapping polygons this can mislabel a point,
so treat a surprising result as a prompt to look at the board, not as proof.
"""
import json
import sys


def inside(pt, ring):
    x, y = pt
    hit = False
    n = len(ring)
    for i in range(n):
        x1, y1 = ring[i]
        x2, y2 = ring[(i + 1) % n]
        if (y1 > y) != (y2 > y) and x < (x2 - x1) * (y - y1) / (y2 - y1) + x1:
            hit = not hit
    return hit


def covered(pt, rings):
    return sum(inside(pt, r) for r in rings) % 2 == 1


def check(path):
    m = json.load(open(path))
    polys = m["polygons"]
    ports = m["ports"]
    print("=====", path)
    bad = 0
    for p in ports:
        pt = (p["x"], p["y"])
        ref = p.get("ref_layer") or "In1.Cu"
        sig = covered(pt, polys.get(p["layer"], []))
        under = covered(pt, polys.get(ref, []))
        flag = "" if (sig and under) else "   <-- NO COPPER: port has no return path"
        bad += 0 if (sig and under) else 1
        print(" port %d %-32s (%.2f, %.2f) type=%-6s | %s copper: %-5s | %s (reference) copper: %-5s%s" % (
            p["number"], p["label"], pt[0], pt[1], p.get("type"), p["layer"], sig, ref, under, flag))
    if len(ports) == 2 and abs(ports[0]["x"] - ports[1]["x"]) < 1e-6:
        x = ports[0]["x"]
        y0, y1 = sorted([ports[0]["y"], ports[1]["y"]])
        ys = [y0 + i * 0.25 for i in range(int((y1 - y0) / 0.25) + 1)]
        ref = ports[0].get("ref_layer") or "In1.Cu"
        strip = "".join("#" if covered((x, y), polys.get(ref, [])) else "." for y in ys)
        print(" reference layer %s along x=%.2f, y %.2f to %.2f, step 0.25 mm:" % (ref, x, y0, y1))
        print("   ", strip)
    print(" RESULT:", "OK, every port has copper under it" if bad == 0 else "%d port(s) with no copper under them" % bad)
    return bad


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    sys.exit(1 if sum(check(p) for p in sys.argv[1:]) else 0)
