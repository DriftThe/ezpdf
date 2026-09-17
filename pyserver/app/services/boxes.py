"""Box geometry, merge and text-dedup primitives.

Pure numpy/PIL-free on purpose: this is the part that gets tuned against real pages, so it must be
exercisable without torch/transformers (tests/test_layout_dedup.py). Every number lives in app/config.py.
"""

from __future__ import annotations

import re
from dataclasses import dataclass, replace
from difflib import SequenceMatcher
from typing import Optional, Sequence

import numpy as np

# Which detection pass produced a box (LayoutDetector.detect): the main pass is the trustworthy one,
# so overlap between passes is resolved in its favour.
ORIGIN_MAIN, ORIGIN_FALLBACK, ORIGIN_TILE, ORIGIN_FIGURE = 0, 1, 2, 3

# Text-family labels: they carry the page's words, so OCR junk (empty content) and near-duplicate
# boxes are resolved for them, and only for them. table/formula already carry the text of everything
# inside them; image/chart are pixels the reader keeps either way.
TEXT_LABELS = frozenset({
    "text", "paragraph_title", "doc_title", "abstract", "aside_text", "footnote",
    "figure_title", "content", "reference", "reference_content", "header", "footer",
    "number", "formula_number", "vision_footnote",
})
STRUCTURED_LABELS = frozenset({"table", "formula"})
FIGURE_LABELS = frozenset({"image", "chart"})

# Merge priority: a structured box owns the text inside it, a figure keeps the text inside it, and
# text boxes resolve among themselves.
_RANK = {label: 0 for label in STRUCTURED_LABELS}
_RANK.update({label: 1 for label in FIGURE_LABELS})
_RANK_TEXT = 2

# A 2-3 char fragment shared with a longer text is a coincidence, not a duplicate ("3" in "31").
_MIN_CONTAINMENT_CHARS = 8
# How much of a text its extracted words have to explain for the word-overlap rule to apply at all.
_MIN_TOKEN_COVERAGE = 0.6
_TOKEN_RE = re.compile(r"[0-9a-z]+")


@dataclass
class LayoutBox:
    xyxy: np.ndarray  # float32, shape (4,) — [x1, y1, x2, y2] in source-image coordinates
    label_id: int
    label_name: str
    score: float
    origin: int = ORIGIN_MAIN

    @property
    def int_rect(self) -> tuple[int, int, int, int]:
        return tuple(int(round(float(v))) for v in self.xyxy)

    @property
    def area(self) -> float:
        x1, y1, x2, y2 = self.xyxy
        return max(0.0, x2 - x1) * max(0.0, y2 - y1)

    def shifted(self, dx: float, dy: float) -> "LayoutBox":
        """Same box moved by (dx, dy) — used to lift a figure's inner boxes into page coordinates."""
        return replace(self, xyxy=self.xyxy + np.array([dx, dy, dx, dy], dtype=np.float32))


@dataclass
class RegionResult:
    label: str
    score: float
    rect: tuple[int, int, int, int]
    markdown: str


@dataclass
class PageResult:
    width: int
    height: int
    elapsed_seconds: float
    regions: list[RegionResult]


def iou_xyxy(a: np.ndarray, b: np.ndarray) -> float:
    """Intersection over union — near-identical boxes."""
    ax1, ay1, ax2, ay2 = a
    bx1, by1, bx2, by2 = b
    iw = max(0.0, min(ax2, bx2) - max(ax1, bx1))
    ih = max(0.0, min(ay2, by2) - max(ay1, by1))
    inter = iw * ih
    union = (ax2 - ax1) * (ay2 - ay1) + (bx2 - bx1) * (by2 - by1) - inter
    return inter / union if union > 0 else 0.0


def containment(a: np.ndarray, b: np.ndarray) -> float:
    """Fraction of ``a`` covered by ``b`` (inter / area(a)) — "this box re-detects a region of that one"."""
    iw = max(0.0, min(a[2], b[2]) - max(a[0], b[0]))
    ih = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    area = max(0.0, a[2] - a[0]) * max(0.0, a[3] - a[1])
    return (iw * ih) / area if area > 0 else 0.0


def iom(a: np.ndarray, b: np.ndarray) -> float:
    """Intersection over the smaller area — symmetric "same region", unlike containment."""
    iw = max(0.0, min(a[2], b[2]) - max(a[0], b[0]))
    ih = max(0.0, min(a[3], b[3]) - max(a[1], b[1]))
    smaller = min(
        max(0.0, a[2] - a[0]) * max(0.0, a[3] - a[1]),
        max(0.0, b[2] - b[0]) * max(0.0, b[3] - b[1]),
    )
    return (iw * ih) / smaller if smaller > 0 else 0.0


def union_xyxy(a: np.ndarray, b: np.ndarray) -> np.ndarray:
    return np.array(
        [min(a[0], b[0]), min(a[1], b[1]), max(a[2], b[2]), max(a[3], b[3])],
        dtype=np.float32,
    )


def split_tiles(
        width: int,
        height: int,
        overlap: float,
        min_height: int,
) -> list[tuple[int, int, int, int]]:
    """Top/bottom half-page crop rects, overlapping by ``overlap`` of the height, in page pixels.

    Returns the rects rather than only their offsets on purpose: the caller crops with them, hands the
    detector the rect's size as the target size and shifts the boxes back by the rect's origin, so all
    three live in one space by construction. A tile cut from a downscaled copy while its boxes were
    shifted in the page's coordinates (an A4 page is 1684 px tall at the render scale 2.0, so it gets
    shrunk to fit ``max_long_side``) comes back displaced by the downscale factor — up to 42 pt up and
    to the left, growing with the distance from the origin. Pages shorter than ``min_height`` yield
    nothing: there is no half-page to scan.
    """
    if height < min_height:
        return []
    cut = height // 2
    margin = int(height * overlap)
    return [
        (0, 0, width, min(height, cut + margin)),
        (0, max(0, cut - margin), width, height),
    ]


class BoxFilter:
    """Threshold + merge + trim for one page's detections (numbers in app/config.py).

    One greedy pass replaces a score-NMS plus a nesting pass: candidates are visited in priority order
    (structured > figure > text, then earlier pass, then larger, then higher score) and dropped when
    they re-detect a kept box — near-identical (IoU) or mostly covered by it. A dropped box is unioned
    into its survivor, so no pixel it covered leaves the crop and the OCR text cannot be lost.

    With ``ink`` (the page's dark-pixel mask) every box is first pulled tight around the ink it
    contains: the detector's boxes carry a margin (median 5-9 px at render scale 2.0 on a paper page)
    and its spurious boxes carry far more, which is what makes two unrelated blocks overlap and what
    the covers then paint over each other. A box with almost no ink inside keeps its rectangle — the
    measurement is not trustworthy there.
    """

    def __init__(
            self,
            iou_threshold: float,
            containment_threshold: float,
            structured_containment_threshold: float,
            min_area: float,
            min_score: float,
            expand_pixels: float,
            trim_margin: float,
            trim_min_area_ratio: float,
            unclip_ratio: float = 0.0,
    ) -> None:
        self.iou_threshold = float(iou_threshold)
        self.containment_threshold = float(containment_threshold)
        self.structured_containment_threshold = float(structured_containment_threshold)
        self.min_area = float(min_area)
        self.min_score = float(min_score)
        self.expand_pixels = float(expand_pixels)
        self.trim_margin = float(trim_margin)
        self.trim_min_area_ratio = float(trim_min_area_ratio)
        self.unclip_ratio = float(unclip_ratio)

    def _unclip(self, box: np.ndarray) -> np.ndarray:
        x1, y1, x2, y2 = box
        dx = (x2 - x1) * self.unclip_ratio + self.expand_pixels
        dy = (y2 - y1) * self.unclip_ratio + self.expand_pixels
        return np.array([x1 - dx, y1 - dy, x2 + dx, y2 + dy], dtype=np.float32)

    def _trim(self, box: LayoutBox, ink: np.ndarray) -> LayoutBox:
        """Pull the rectangle tight around the ink inside it, keeping ``trim_margin`` px of slack."""
        x1, y1, x2, y2 = box.int_rect
        x1, y1 = max(0, x1), max(0, y1)
        x2, y2 = min(ink.shape[1], x2), min(ink.shape[0], y2)
        if x2 - x1 < 2 or y2 - y1 < 2:
            return box
        patch = ink[y1:y2, x1:x2]
        rows = np.flatnonzero(patch.any(axis=1))
        cols = np.flatnonzero(patch.any(axis=0))
        if rows.size == 0 or cols.size == 0:
            return box
        margin = self.trim_margin
        tight = np.array(
            [
                max(float(x1), x1 + cols[0] - margin),
                max(float(y1), y1 + rows[0] - margin),
                min(float(x2), x1 + cols[-1] + 1 + margin),
                min(float(y2), y1 + rows[-1] + 1 + margin),
            ],
            dtype=np.float32,
        )
        if (tight[2] - tight[0]) * (tight[3] - tight[1]) < self.trim_min_area_ratio * box.area:
            return box
        return replace(box, xyxy=tight)

    def _expanded(
            self,
            box: LayoutBox,
            page_size: Optional[tuple[int, int]],
            ink: Optional[np.ndarray],
    ) -> LayoutBox:
        """Final geometry in one place: grow a little (proportional unclip is off by default: it
        reaches into small headings and the covers then overlap), clamp to the page, trim to ink.
        The merge below judges exactly the rect that gets cropped and drawn."""
        rect = self._unclip(box.xyxy).copy()
        if page_size is not None:
            w, h = page_size
            rect[0] = max(0.0, rect[0])
            rect[1] = max(0.0, rect[1])
            rect[2] = min(float(w), rect[2])
            rect[3] = min(float(h), rect[3])
        expanded = replace(box, xyxy=rect)
        if ink is None or self.trim_margin < 0:
            return expanded
        return self._trim(expanded, ink)

    @staticmethod
    def _priority(box: LayoutBox) -> tuple[int, int, float, float]:
        return (_RANK.get(box.label_name, _RANK_TEXT), box.origin, -box.area, -box.score)

    def _absorbs(self, candidate: LayoutBox, kept: LayoutBox) -> bool:
        """True when ``candidate`` re-detects ``kept`` (candidate is dropped and unioned into it)."""
        if iou_xyxy(candidate.xyxy, kept.xyxy) > self.iou_threshold:
            return True
        # A figure keeps the text detected inside it: that text is what the reader wants translated.
        if kept.label_name in FIGURE_LABELS and candidate.label_name not in FIGURE_LABELS:
            return False
        limit = (
            self.structured_containment_threshold
            if kept.label_name in STRUCTURED_LABELS
            else self.containment_threshold
        )
        return containment(candidate.xyxy, kept.xyxy) > limit

    def filter(
            self,
            boxes: Sequence[LayoutBox],
            page_size: Optional[tuple[int, int]] = None,
            ink: Optional[np.ndarray] = None,
    ) -> list[LayoutBox]:
        """Drop junk by score/area, merge duplicates; returns kept boxes in score order."""
        expanded = [self._expanded(b, page_size, ink) for b in boxes]
        candidates = [
            b for b in expanded if b.score >= self.min_score and b.area >= self.min_area
        ]
        kept: list[LayoutBox] = []
        for b in sorted(candidates, key=self._priority):
            survivor = next((i for i, k in enumerate(kept) if self._absorbs(b, k)), None)
            if survivor is None:
                kept.append(b)
            else:
                kept[survivor] = replace(kept[survivor], xyxy=union_xyxy(kept[survivor].xyxy, b.xyxy))
        return sorted(kept, key=lambda b: b.score, reverse=True)


def has_words(markdown: str, min_chars: int = 1) -> bool:
    """False for a box whose OCR found nothing but punctuation or a stray glyph.

    A paragraph-sized detection that reads as one character is a mis-detection, not content — but a
    figure label really can be two characters ("R3"), so the floor is a knob, and the default only
    asks for a single letter or digit (the empty and ``"___"`` cases).
    """
    return sum(1 for ch in markdown if ch.isalnum()) >= max(1, min_chars)


def is_placeholder(markdown: str, placeholders: "Sequence[str]") -> bool:
    """True for the VL's own "I could not read anything here" marker (``[Unlabeled]`` and friends)."""
    return _normalized(markdown) in {_normalized(p) for p in placeholders}


def looks_like_body_text(
        markdown: str,
        *,
        min_chars: int,
        min_lines: int,
        min_alnum_ratio: float,
) -> bool:
    """Does a figure's own OCR read like a body of text rather than a stray label or a photo?

    Half of the figure gate (see OCRPipeline._figure_regions): a photo yields nothing or an invented
    one-liner, a diagram yields the several lines of labels the model actually read. The question is
    answered by the model that already looked at the pixels — ink density points the wrong way, since
    a photo has more ink than a diagram.
    """
    stripped = "".join(markdown.split())
    if len(stripped) < min_chars:
        return False
    if len([line for line in markdown.splitlines() if line.strip()]) < min_lines:
        return False
    alnum = sum(1 for ch in stripped if ch.isalnum())
    return alnum / len(stripped) >= min_alnum_ratio


def _normalized(markdown: str) -> str:
    return "".join(ch.lower() for ch in markdown if ch.isalnum())


def _tokens(markdown: str) -> set[str]:
    """Word set of a spaced script. CJK has no separators, so it yields almost nothing — the callers
    check how much of the text the tokens actually explain before trusting an overlap."""
    return set(_TOKEN_RE.findall(markdown.lower()))


def _fuzzy_hit(token: str, others: set[str], prefix: int) -> bool:
    """OCR truncates the tail of a word far more often than it invents one ("connota"/"connotation",
    "service"/"services"), so a shared head counts as the same word."""
    if token in others:
        return True
    if prefix <= 0 or len(token) < prefix:
        return False
    head = token[:prefix]
    return any(len(other) >= prefix and other[:prefix] == head for other in others)


def texts_look_dual(
        a: str,
        b: str,
        *,
        ratio: float,
        token_containment: float,
        fuzzy_prefix: int = 0,
) -> bool:
    """Near-duplicate OCR text — the same words read twice, possibly with a truncated tail.

    Deliberately conservative: different text keeps both boxes, because a wrongly dropped block costs
    the reader a translation while a leftover duplicate only costs ink.
    """
    na, nb = _normalized(a), _normalized(b)
    if not na or not nb:
        return False
    if na == nb:
        return True
    short, long = (na, nb) if len(na) <= len(nb) else (nb, na)
    if len(short) >= _MIN_CONTAINMENT_CHARS and short in long:
        return True
    # Word overlap helps only where words exist; a CJK paragraph with a stray Latin term must not look
    # like a duplicate of every other one, hence the coverage floor.
    tokens_a = _tokens(a)
    tokens_b = _tokens(b)
    if tokens_a and tokens_b:
        smaller, larger = (tokens_a, tokens_b) if len(tokens_a) <= len(tokens_b) else (tokens_b, tokens_a)
        shared = sum(1 for t in smaller if _fuzzy_hit(t, larger, fuzzy_prefix)) / len(smaller)
        coverage = min(
            sum(map(len, tokens_a)) / len(na),
            sum(map(len, tokens_b)) / len(nb),
        )
        if shared >= token_containment and coverage >= _MIN_TOKEN_COVERAGE and (
                min(len(tokens_a), len(tokens_b)) >= 3
                or (shared == 1.0 and len(short) >= _MIN_CONTAINMENT_CHARS)):
            return True
    return SequenceMatcher(None, na, nb).ratio() >= ratio


@dataclass
class PageRegions:
    """One page's post-OCR regions, plus what was taken out so the caller can audit the merge.

    ``merged`` are near-duplicates folded into a survivor (their text is the survivor's text read a
    second time), ``junk`` are boxes whose reading was punctuation, a stray glyph or the VL's own
    placeholder. Both are empty on a page that needed neither.
    """

    regions: list[tuple[LayoutBox, str]]
    merged: list[tuple[LayoutBox, str]]
    junk: list[LayoutBox]


def resolve_text_duplicates(
        items: "Sequence[tuple[LayoutBox, str]]",
        *,
        min_overlap: float,
        ratio: float,
        token_containment: float,
        fuzzy_prefix: int = 0,
) -> list[tuple[int, LayoutBox, str]]:
    """Merge near-duplicate text regions on one page: overlap AND near-equal OCR text are both required.

    Returns ``(position, box, text)`` for each survivor, ``position`` being its index in ``items`` so a
    caller mixing text and non-text regions can restore the page's original order. Boxes are visited by
    completeness of their text (then score, then area), so a truncated OCR never replaces the full one;
    the survivor's rect is unioned with every box it absorbs. Boxes whose text differs are all kept —
    the two recall passes are why this runs after OCR, where the text can arbitrate between boxes that
    geometry alone cannot tell apart.
    """
    order = sorted(
        range(len(items)),
        key=lambda i: (-len(_normalized(items[i][1])), -items[i][0].score, -items[i][0].area),
    )
    survivors = list(items)
    absorbed: set[int] = set()
    for pos, i in enumerate(order):
        if i in absorbed:
            continue
        box, text = survivors[i]
        for j in order[pos + 1:]:
            if j in absorbed:
                continue
            other, other_text = survivors[j]
            if iom(box.xyxy, other.xyxy) < min_overlap:
                continue
            if not texts_look_dual(
                text, other_text, ratio=ratio, token_containment=token_containment,
                fuzzy_prefix=fuzzy_prefix,
            ):
                continue
            absorbed.add(j)
            box = replace(box, xyxy=union_xyxy(box.xyxy, other.xyxy))
            text = text if len(_normalized(text)) >= len(_normalized(other_text)) else other_text
        survivors[i] = (box, text)
    return [
        (pos, entry[0], entry[1])
        for pos, entry in enumerate(survivors)
        if pos not in absorbed
    ]


def resolve_page_regions(
        items: "Sequence[tuple[LayoutBox, str]]",
        *,
        min_overlap: float,
        ratio: float,
        token_containment: float,
        fuzzy_prefix: int = 0,
        min_content_chars: int = 1,
        placeholders: "Sequence[str]" = (),
) -> PageRegions:
    """One page's post-OCR cleanup, keeping the original region order.

    Drops ghost text boxes (a reading that is punctuation, a stray glyph or the VL's placeholder) and
    merges near-duplicate text regions. Everything that is not text-family — tables, formulas, figures,
    seals — passes through untouched: a table carries its own text, and a figure is pixels the reader
    keeps either way.
    """
    survivors: dict[int, tuple[LayoutBox, str]] = {}
    text_items: list[tuple[int, LayoutBox, str]] = []
    junk: list[LayoutBox] = []
    for index, (box, markdown) in enumerate(items):
        if box.label_name not in TEXT_LABELS:
            survivors[index] = (box, markdown)
        elif has_words(markdown, min_content_chars) and not is_placeholder(markdown, placeholders):
            text_items.append((index, box, markdown))
        else:
            junk.append(box)
    resolved = resolve_text_duplicates(
        [(box, markdown) for _, box, markdown in text_items],
        min_overlap=min_overlap,
        ratio=ratio,
        token_containment=token_containment,
        fuzzy_prefix=fuzzy_prefix,
    )
    merged_positions = {pos for pos, _, _ in resolved}
    for pos, box, markdown in resolved:
        survivors[text_items[pos][0]] = (box, markdown)
    return PageRegions(
        regions=[survivors[index] for index in sorted(survivors)],
        merged=[
            (box, markdown)
            for pos, (_, box, markdown) in enumerate(text_items)
            if pos not in merged_positions
        ],
        junk=junk,
    )
