"""Generate the original 320px LCD background loops (NumPy and Pillow)."""

from pathlib import Path
import sys

import numpy as np
from PIL import Image

SIZE = 320
FRAMES = 48
PERIOD_MS = 4000
TAU = np.pi * 2
axis = (np.arange(SIZE, dtype=np.float32) + 0.5) / SIZE
x, y = np.meshgrid(axis, axis)
dx, dy = x - 0.5, y - 0.5
radius = np.hypot(dx, dy)
angle = np.arctan2(dy, dx)
vignette = np.clip(1.16 - radius, 0, 1)


def palette(level, *colors):
    rgb = np.array(colors, dtype=np.float32)
    index = np.clip(level, 0, 1) * (len(rgb) - 1)
    low = np.minimum(index.astype(int), len(rgb) - 2)
    return rgb[low] + (rgb[low + 1] - rgb[low]) * (index - low)[..., None]


def add(image, intensity, color):
    image += np.asarray(intensity)[..., None] * np.array(color)


def glow(distance, width):
    return np.exp(-np.square(distance / width))


def points(seed, count):
    return np.random.default_rng(seed).random((count, 4))


STARS = points(11, 64)
DROPS = points(29, 22)
PETALS = points(92, 24)
CELLS = points(34, 24)


def stars(p):
    light = np.zeros_like(x)
    for sx, sy, size, phase in STARS:
        blink = 0.33 + 0.27 * np.cos(p + phase * TAU)
        light += glow(np.hypot(x - sx, y - sy), 0.002 + 0.002 * size) * blink
    return light


def terrain(p):
    return (
        np.sin(7 * x + 3 * y + np.sin(p))
        + np.sin(9 * y - 3 * x + 0.6 * np.cos(p))
        + 0.55 * np.sin(15 * x - 12 * y - 0.5 * np.sin(p))
    ) / 5 + 0.5


def aurora_curtain(p):
    image = palette(y, (6, 12, 36), (9, 20, 32), (11, 42, 42))
    add(image, stars(p) * (1 - 0.65 * y), (185, 218, 234))
    for index in range(3):
        fold = np.sin(7 * x + index * 0.9 + 0.55 * np.sin(p)) * 0.07
        edge = 0.37 + index * 0.10 + fold + 0.03 * np.cos(p + index)
        curtain = np.maximum(0, edge - y)
        rays = 0.7 + 0.3 * np.sin(64 * x + np.sin(13 * x + p) * 2)
        light = (glow(edge - y, 0.014) + np.exp(-14 * curtain)) * (y < edge)
        add(image, light * rays * vignette * 0.65, [(38, 169, 155), (85, 67, 178), (22, 111, 130)][index])
    return image


def contour_map(p):
    height = terrain(p)
    image = palette(height, (9, 35, 40), (25, 77, 62), (76, 120, 94), (177, 154, 97))
    contours = np.abs(np.mod(height * 15 + 0.5, 1) - 0.5)
    add(image, glow(contours, 0.065) * 0.65, (100, 103, 70))
    return image


def deep_space(p):
    dust = terrain(0.6)
    drift = 0.09 * np.sin(p)
    cloud = np.exp(-np.square((dy - 0.26 * np.sin(dx * 7 + drift)) * 5))
    image = palette(cloud * dust * vignette, (4, 8, 24), (45, 28, 102), (119, 62, 156))
    add(image, stars(p), (213, 203, 252))
    return image


def digital_rain(p):
    column = (x * 27).astype(int)
    cell = np.mod(np.floor(y * 33 - p / TAU * 33).astype(int), 33)
    shaft = np.mod(y * 2 - p / TAU + column * 0.414, 1)
    code = np.mod(column * 173 + cell * 17 + cell * column, 9)
    stroke = (np.mod(x * 27, 1) > 0.25) & (np.mod(y * 33, 1) < 0.61)
    slots = (code > 2) & stroke
    return palette((1 - shaft) ** 4 * (0.12 + 0.88 * slots), (3, 12, 15), (14, 102, 79), (146, 247, 198))


def gradient_drift(p):
    image = palette(y, (10, 15, 41), (32, 16, 51))
    for phase, color in [(0, (46, 99, 208)), (2.1, (192, 52, 126)), (4.2, (38, 159, 146))]:
        center = np.hypot(dx - 0.34 * np.cos(p + phase), dy - 0.34 * np.sin(p + phase))
        add(image, glow(center, 0.33) * vignette, color)
    return image


def holo_foil(p):
    facets = np.floor((x + y * 0.58) * 21) - np.floor((x - y * 0.43) * 18)
    reflection = np.sin(TAU * (0.7 * x + 0.25 * y) + p + facets * 0.38) * 0.5 + 0.5
    image = palette(reflection, (29, 26, 54), (99, 77, 146), (71, 173, 180), (252, 216, 205))
    add(image, glow(reflection - 0.82, 0.13) * 0.25, (187, 217, 240))
    return image


def ink_wash(p):
    height = terrain(p)
    tide = 0.51 + 0.12 * np.cos(p)
    paper = np.sin(113 * x + 73 * y) * np.sin(147 * x - 33 * y)
    wash = np.clip((height - tide + paper * 0.012) * 6, 0, 1)
    image = palette(wash, (223, 208, 178), (170, 148, 127), (85, 74, 84), (22, 26, 39))
    add(image, -glow(height - tide - 0.135, 0.023) * 0.27, (95, 92, 85))
    return image


def lava_lamp(p):
    vessel = np.clip((0.39 - np.abs(dx)) * 54, 0, 1)
    density = np.zeros_like(x)
    for index in range(6):
        phase = p + index * TAU / 6
        cx = np.sin(phase * 2 + index) * 0.22
        cy = 0.53 - np.mod(phase / TAU, 1) * 1.06
        distance = np.hypot(dx - cx, (dy - cy) * 0.83)
        density += glow(distance, 0.089 + 0.024 * np.cos(index))
    wax = np.clip((density - 0.23) * 3, 0, 1)
    image = palette(wax * vessel, (12, 11, 26), (69, 16, 48), (169, 39, 49), (247, 139, 66))
    add(image, glow(np.abs(dx) - 0.39, 0.015) * 0.3, (87, 100, 142))
    return image


def liquid_metal(p):
    folds = 10 * dx + 4 * np.sin(8 * dy + 1.3 * np.sin(p))
    surface = np.sin(folds + np.cos(dx * 17 + dy * 11 + p)) * 0.5 + 0.5
    reflected = np.power(surface, 5)
    return palette(reflected, (9, 21, 33), (59, 97, 112), (178, 209, 208), (240, 231, 205))


def marble_ink(p):
    fold_x = x + 0.11 * np.sin(12 * y + 0.7 * np.cos(p))
    fold_y = y + 0.13 * np.sin(8 * fold_x + 0.5 * np.sin(p))
    ink = (np.sin(32 * fold_x + 12 * fold_y + 3 * np.sin(13 * fold_y)) + 1) * 0.5
    return palette(ink ** 8, (220, 210, 178), (185, 152, 124), (59, 99, 103), (25, 33, 49))


def nebula_drift(p):
    image = palette(y, (10, 9, 34), (11, 21, 45))
    for phase, hue in [(0, (105, 35, 105)), (2.1, (25, 86, 145)), (4.2, (150, 58, 61))]:
        u = dx - 0.27 * np.sin(p + phase)
        v = dy - 0.23 * np.cos(p + phase)
        d = np.hypot(u + 0.06 * np.sin(v * 18), v + 0.06 * np.sin(u * 15))
        add(image, glow(d, 0.20) * (0.5 + 0.25 * np.sin(u * 24 + v * 13)), hue)
    add(image, stars(p) * 0.62, (170, 186, 233))
    return image


def neon_rain(p):
    image = palette(y, (7, 11, 29), (21, 12, 41))
    for index, (sx, sy, length, _) in enumerate(DROPS):
        speed = 1 + index % 2
        head = np.mod(sy + p / TAU * speed * 1.34, 1.34) - 0.17
        trail = head - y
        light = glow(x - sx - dy * 0.12, 0.006) * np.exp(-np.maximum(trail, 0) * (13 + length * 12))
        add(image, light * (trail >= 0), (43, 159, 196) if index % 2 else (149, 51, 123))
    return image


def ocean_depths(p):
    image = palette(y, (6, 68, 87), (9, 35, 61), (6, 13, 36))
    rays = np.zeros_like(y)
    for index in range(4):
        width = 0.014 + y * 0.04
        path = 0.21 + index * 0.13 + y * (index - 2) * 0.15
        rays += glow(x - path - 0.032 * np.sin(p + index), width) * (1 - y) ** 1.5
    add(image, rays * 0.55, (60, 124, 129))
    for sx, sy, speed, phase in STARS[::2]:
        head = np.mod(sy - p / TAU * 1.16, 1.16) - 0.08
        add(image, glow(np.hypot(x - sx - 0.028 * np.sin(p + phase * TAU), y - head), 0.004 + speed * 0.003) * 0.38, (85, 175, 166))
    return image


def plasma_flow(p):
    folds = np.sin(10 * x + 5 * y + 1.2 * np.cos(p))
    field = np.sin(7 * y + 7 * x * folds + 2 * np.sin(p)) + np.sin(9 * x - 10 * y + p)
    return palette((np.sin(field * 1.3 + 0.6 * np.sin(p)) * 0.5 + 0.5) ** 2, (10, 13, 48), (49, 40, 143), (155, 48, 179), (249, 124, 149))


def pool(p):
    a = 34 * x + 2.8 * np.sin(10 * y + p)
    b = 26 * y + 3.4 * np.sin(11 * x - p)
    lattice = np.minimum(np.abs(np.sin(a + b)), np.abs(np.sin(a - b)))
    image = palette(terrain(p), (10, 58, 79), (19, 124, 135), (54, 172, 159))
    add(image, glow(lattice, 0.19) * 0.78, (95, 139, 112))
    return image


def rain_glass(p):
    image = palette(terrain(0), (14, 21, 34), (52, 33, 54), (66, 72, 81))
    for sx, sy, size, phase in DROPS:
        head = np.mod(sy + p / TAU * 1.38, 1.38) - 0.17
        offset = x - sx - 0.008 * np.sin(11 * y + phase * TAU)
        streak = glow(offset, 0.003 + size * 0.004) * (y < head) * np.exp(-np.abs(y - head) * 11)
        drop = np.hypot(offset, (y - head) * 0.63)
        add(image, glow(drop - 0.006 - size * 0.007, 0.003) * 0.7 + streak * 0.35, (104, 117, 153))
        add(image, -glow(drop, 0.009) * 0.36, (54, 46, 38))
    return image


def retro_sunset(p):
    horizon = 0.61
    sky = palette(y / horizon, (13, 14, 46), (74, 25, 86), (157, 41, 106))
    sun_d = np.hypot(dx, y - 0.43)
    disk = np.clip((0.205 - sun_d) * 270, 0, 1)
    scan = np.where(y < 0.42, 1, np.clip((np.mod(y * 58, 1) - 0.20) * 8, 0, 1))
    sun = palette(y * 1.65, (230, 114, 93), (255, 195, 106), (226, 66, 109))
    sky += (sun - sky) * (disk * scan)[..., None]
    depth = np.maximum(y - horizon, 0.011)
    floor = palette(depth * 2.5, (86, 26, 94), (16, 12, 39))
    rails = np.abs(np.mod(dx / depth * 2 + 0.5, 1) - 0.5)
    ties = np.abs(np.mod(1.1 / depth + p / TAU * 4 + 0.5, 1) - 0.5)
    add(floor, glow(np.minimum(rails, ties), 0.055) * (0.3 + depth), (181, 60, 142))
    return np.where((y < horizon)[..., None], sky, floor)


def sakura_drift(p):
    image = palette(y, (30, 20, 58), (86, 45, 86), (144, 81, 103))
    for sx, sy, size, phase in PETALS:
        cx = sx + 0.08 * np.sin(p + phase * TAU)
        cy = np.mod(sy + p / TAU * 1.16, 1.16) - 0.08
        a = p + phase * TAU
        u = (x - cx) * np.cos(a) + (y - cy) * np.sin(a)
        v = -(x - cx) * np.sin(a) + (y - cy) * np.cos(a)
        height = 0.006 + 0.012 * size
        d = np.hypot(u / (height * 0.55), v / height)
        add(image, np.clip((1 - d) * 4, 0, 1) * (0.46 + 0.32 * np.cos(a)), (169, 123, 108))
    return image


def tesseract(p):
    image = palette(radius * 1.4, (17, 26, 59), (8, 14, 34))
    a = 0.25 * np.sin(p)
    u = dx * np.cos(a) + dy * np.sin(a)
    v = -dx * np.sin(a) + dy * np.cos(a)
    for index in range(8):
        depth = np.mod(index / 8 - p / TAU, 1)
        extent = 0.08 + 0.52 * depth ** 1.7
        rim = np.abs(np.maximum(np.abs(u + 0.10 * depth), np.abs(v - 0.05 * depth)) - extent)
        energy = np.sin(np.pi * depth) ** 2
        add(image, (glow(rim, 0.003) + glow(rim, 0.012) * 0.24) * energy, (59, 178, 196))
    return image


def voronoi_glass(p):
    nearest = np.full_like(x, 3)
    runner_up = nearest.copy()
    winner = np.zeros_like(x)
    for index, (sx, sy, _, phase) in enumerate(CELLS):
        d = (x - sx - 0.036 * np.sin(p + phase * TAU)) ** 2 + (y - sy - 0.036 * np.cos(p + index)) ** 2
        runner_up = np.minimum(runner_up, np.maximum(nearest, d))
        winner = np.where(d < nearest, index / len(CELLS), winner)
        nearest = np.minimum(nearest, d)
    image = palette(np.mod(winner * 4.27, 1), (12, 45, 72), (39, 128, 141), (86, 73, 139), (173, 99, 80), (189, 169, 91))
    bevel = glow(runner_up - nearest, 0.0026)
    return image * (0.88 + 0.12 * np.cos(nearest * 140))[..., None] * (1 - bevel * 0.8)[..., None]


def warp_tunnel(p):
    depth = -np.log(np.maximum(radius, 0.018)) * 1.45
    sector = (angle + depth * 0.38 + 0.19 * np.sin(p)) * 7 / TAU
    ribs = np.abs(np.mod(depth + p / TAU * 2 + 0.5, 1) - 0.5)
    struts = np.abs(np.mod(sector + 0.5, 1) - 0.5)
    image = palette(np.mod(sector, 1) * 0.28 + radius * 0.35, (8, 13, 27), (42, 59, 111), (70, 56, 106))
    add(image, glow(ribs, 0.094) * np.clip(radius * 2.3, 0, 0.8), (178, 74, 150))
    add(image, glow(struts, 0.044) * np.clip(radius * 2.3, 0, 0.8), (68, 126, 160))
    return image


BACKGROUNDS = (
    aurora_curtain, contour_map, deep_space, digital_rain, gradient_drift, holo_foil,
    ink_wash, lava_lamp, liquid_metal, marble_ink, nebula_drift, neon_rain,
    ocean_depths, plasma_flow, pool, rain_glass, retro_sunset, sakura_drift,
    tesseract, voronoi_glass, warp_tunnel,
)
TRUECOLOR = {deep_space, gradient_drift, nebula_drift, sakura_drift}


def main():
    directory = Path(__file__).resolve().parents[1] / "resources" / "lcd" / "backgrounds"
    directory.mkdir(exist_ok=True)
    selected = set(sys.argv[1:])
    available = {render.__name__.replace("_", "-") for render in BACKGROUNDS}
    if selected - available:
        raise SystemExit("Unknown backgrounds: " + ", ".join(sorted(selected - available)))
    for render in BACKGROUNDS:
        name = render.__name__.replace("_", "-")
        if selected and name not in selected:
            continue
        frames = []
        for index in range(FRAMES):
            image = render(TAU * index / FRAMES) * (0.70 + 0.32 * vignette[..., None])
            frame = np.clip(np.round(image), 0, 255).astype(np.uint8)
            frames.append(Image.fromarray(frame))
        if render not in TRUECOLOR:
            # Keep shade assignments consistent throughout the animation.
            samples = Image.new("RGB", (4 * SIZE, 2 * SIZE))
            for index, frame in enumerate(frames[::6]):
                samples.paste(frame, (index % 4 * SIZE, index // 4 * SIZE))
            colors = samples.quantize(colors=256)
            frames = [frame.quantize(palette=colors, dither=Image.Dither.NONE) for frame in frames]
        durations = [round(PERIOD_MS * i / FRAMES) for i in range(FRAMES + 1)]
        path = directory / (name + ".png")
        frames[0].save(path, save_all=True, append_images=frames[1:], loop=0,
                       duration=np.diff(durations).tolist(), compress_level=7)
        print(path.name, path.stat().st_size, flush=True)


if __name__ == "__main__":
    main()
