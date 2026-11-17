#!/usr/bin/env python3
"""Pixel comparators for PPM captures of an output with a pure blue background.

  pix.py bbox  FILE X Y W H      -> "x=.. y=.. w=.. h=.. n=.." of the non-background pixels in the region
  pix.py halo  FILE X Y W H      -> "edge=.. bad=.." for translucent red/blue edge pixels off the expected blend line
  pix.py stripes FILE X Y W H    -> "ratio=.." fraction of horizontal neighbours with full contrast on the middle row
  pix.py diff  A B               -> "diff=.." number of differing pixels
"""
import sys

BG = (0, 0, 255)


def load(path):
    magic, size, _maxval, pixels = open(path, "rb").read().split(b"\n", 3)
    width, height = map(int, size.split())
    return width, pixels, height


def region(path, args):
    width, pixels, height = load(path)
    x, y, w, h = (int(a) for a in args)
    for py in range(max(y, 0), min(y + h, height)):
        for px in range(max(x, 0), min(x + w, width)):
            i = (py * width + px) * 3
            yield px, py, pixels[i], pixels[i + 1], pixels[i + 2]


def bbox(path, args):
    hits = [(x, y) for x, y, r, g, b in region(path, args) if (r, g, b) != BG]
    if not hits:
        return "x=0 y=0 w=0 h=0 n=0"
    xs, ys = [p[0] for p in hits], [p[1] for p in hits]
    return f"x={min(xs)} y={min(ys)} w={max(xs) - min(xs) + 1} h={max(ys) - min(ys) + 1} n={len(hits)}"


def halo(path, args):
    # The test discs are (255, g, 60) with straight alpha t over blue: r = 255t and b = 255 - 195t.
    edge = bad = 0
    for _x, _y, r, _g, b in region(path, args):
        if 8 < r < 247:
            edge += 1
            bad += abs(b - (255 - 195 * r / 255)) > 40
    return f"edge={edge} bad={bad}"


def stripes(path, args):
    x, y, w, h = (int(a) for a in args)
    row = [r for _x, py, r, _g, _b in region(path, (x, y + h // 2, w, 1))]
    pairs = list(zip(row, row[1:]))
    good = sum(abs(a - b) >= 200 for a, b in pairs)
    return f"ratio={good / max(len(pairs), 1):.3f}"


def diff(path, args):
    a, b = load(path), load(args[0])
    if a[0] != b[0] or len(a[1]) != len(b[1]):
        return "diff=-1"
    return f"diff={sum(a[1][i:i + 3] != b[1][i:i + 3] for i in range(0, len(a[1]), 3))}"


def warm(path, args):
    # Pixels of the test discs (255, g, 60): neither the blue background nor a black window has them.
    return f"n={sum(1 for _x, _y, r, _g, b in region(path, args) if r > 200 and b < 120)}"


COMMANDS = {"bbox": bbox, "halo": halo, "stripes": stripes, "diff": diff, "warm": warm}

if __name__ == "__main__":
    print(COMMANDS[sys.argv[1]](sys.argv[2], sys.argv[3:]))
