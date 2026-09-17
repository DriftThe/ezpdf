"""Text-line segmentation for figure crops.

PaddleOCR-VL reads a dense figure's text, but PP-DocLayoutV3 only labels the figure as a whole: it is
a document-layout model, and asking it for the labels inside a diagram finds one or two boxes in any
crop presentation (measured on a 944x314 flow chart: 1 box at 0.5/0.45/0.38 and at four padded aspect
ratios, 4 boxes when the crop is split in half). A morphological scan does see them, because a
rendered diagram's labels are crisp dark runs of glyphs — one horizontal dilation merges a line, and
the ink band separates text from the frames and panels around it (measured 0.17-0.29 of the box for
text, 0.08-0.10 for the diagram's light backdrops and rules).

Pure cv2/numpy so it is testable on its own; the numbers live in app/config.py.
"""

from __future__ import annotations

from typing import Sequence

import cv2
import numpy as np
from PIL import Image

Rect = tuple[int, int, int, int]  # x1, y1, x2, y2 in crop pixels


def ink_mask(image: Image.Image) -> np.ndarray:
    """Bool mask of the dark pixels ("ink"), page sized.

    Otsu on the inverted image: a rendered page, a screenshot and a scanned sheet all separate cleanly
    into ink and background, and both the box trim (boxes.py) and the line scan below want exactly
    that map. Adaptive thresholding turns flat areas into noise, so it is not used.
    """
    gray = cv2.cvtColor(np.asarray(image.convert("RGB")), cv2.COLOR_RGB2GRAY)
    _, binary = cv2.threshold(gray, 0, 255, cv2.THRESH_BINARY_INV + cv2.THRESH_OTSU)
    return binary > 0


def detect_text_lines(
        image: Image.Image,
        *,
        merge_px: int,
        min_height: int,
        max_height: int,
        min_width: int,
        min_ink: float,
        max_ink: float,
) -> list[Rect]:
    """Line-shaped ink runs, in reading order. Borders, rules and filled panels are filtered out.

    ``merge_px`` is the horizontal dilation width: wide enough to join the words of a line, narrow
    enough not to bridge two columns of a diagram.
    """
    binary = ink_mask(image)
    merged = cv2.dilate(
        (binary * 255).astype(np.uint8),
        cv2.getStructuringElement(cv2.MORPH_RECT, (max(1, int(merge_px)), 3)),
        iterations=1,
    )

    count, _, stats, _ = cv2.connectedComponentsWithStats(merged, connectivity=8)
    lines: list[Rect] = []
    for index in range(1, count):
        x, y, w, h = (int(v) for v in stats[index][:4])
        if not (min_height <= h <= max_height and w >= min_width):
            continue
        ink = float(binary[y:y + h, x:x + w].mean())
        if min_ink <= ink <= max_ink:
            lines.append((x, y, x + w, y + h))
    return sorted(lines, key=lambda rect: (rect[1], rect[0]))


def group_lines(
        lines: Sequence[Rect],
        *,
        gap_ratio: float,
        min_x_overlap: float,
) -> list[Rect]:
    """Join the lines of one label into a block (a boxed label is routinely two lines).

    Two lines belong together when the vertical gap between them is smaller than a fraction of their
    height and they start and end in roughly the same place — the same rule a paragraph groups by, so
    a figure's paragraph comes out as one block and its separate labels stay separate.
    """
    groups: list[list[Rect]] = []
    for rect in sorted(lines, key=lambda r: (r[1], r[0])):
        x1, y1, x2, y2 = rect
        host = None
        for group in groups:
            gx1 = min(r[0] for r in group)
            gx2 = max(r[2] for r in group)
            gy2 = max(r[3] for r in group)
            height = max(r[3] - r[1] for r in group)
            overlap = max(0, min(gx2, x2) - max(gx1, x1))
            if y1 - gy2 > gap_ratio * height:
                continue
            if overlap < min_x_overlap * min(gx2 - gx1, x2 - x1):
                continue
            host = group
            break
        if host is None:
            groups.append([rect])
        else:
            host.append(rect)
    return [
        (min(r[0] for r in g), min(r[1] for r in g), max(r[2] for r in g), max(r[3] for r in g))
        for g in groups
    ]
