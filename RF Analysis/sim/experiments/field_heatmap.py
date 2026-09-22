import h5py
import numpy as np
import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt

def load_mag(path):
    f = h5py.File(path, "r")
    E = f["FieldData"]["FD"]["f0"][:]  # (3, nx, ny, nz)
    x = f["Mesh"]["x"][:] * 1000  # m -> mm
    y = f["Mesh"]["y"][:] * 1000
    mag = np.sqrt(np.sum(np.abs(E[:, :, :, 0]) ** 2, axis=0))  # (nx, ny)
    return mag, x, y

cases = [
    ("probe6mm (BROKEN, 6mm)", r"C:\Users\chith\Desktop\telemetry\RF Analysis\results\laptop\step1_probe6mm_fine\exc1\Ef.h5", [(18.26, -34.5), (18.26, -28.5)]),
    ("5mm (good)", r"C:\Users\chith\Desktop\telemetry\RF Analysis\results\laptop\taskC_5mm_w032_base\exc1\Ef.h5", [(18.26, -34.5), (18.26, -29.5)]),
]

fig, axes = plt.subplots(1, 2, figsize=(12, 6))
for ax, (name, path, ports) in zip(axes, cases):
    mag, x, y = load_mag(path)
    # log scale for visibility
    logmag = np.log10(np.maximum(mag, mag.max() * 1e-6))
    im = ax.pcolormesh(x, y, logmag.T, shading="auto", cmap="inferno")
    for px, py in ports:
        ax.plot(px, py, "c+", markersize=15, markeredgewidth=2)
    ax.set_title(name)
    ax.set_xlabel("x (mm)")
    ax.set_ylabel("y (mm)")
    ax.set_aspect("equal")
    plt.colorbar(im, ax=ax, label="log10 |E|")

plt.tight_layout()
out = r"C:\Users\chith\AppData\Local\Temp\field_heatmap.png"
plt.savefig(out, dpi=110)
print("saved", out)
