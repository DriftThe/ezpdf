"""Layout merge / text-dedup regression tests.

The fixtures are real duplicate pairs lifted from a page that came out of the pipeline wrong
(an arXiv paper, pages 1-3 and 8): each pair is one text region the detector reported twice with an
offset, together with the coverage the two rects have on each other. Coordinates are render-scale
pixels (the frontend renders at 2.0, so the pt values from the bound JSON are doubled).

Run from the repository root or from pyserver/:
    python pyserver/tests/test_layout_dedup.py -v
    pyserver/.venv/Scripts/python -m unittest discover -s pyserver/tests
"""

from __future__ import annotations

import sys
import unittest
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.config import (  # noqa: E402
    BOX_EXPAND_PIXELS,
    BOX_MIN_AREA,
    BOX_MIN_SCORE,
    BOX_TRIM_MARGIN_PX,
    BOX_TRIM_MIN_AREA_RATIO,
    DEDUP_CONTAINMENT,
    DEDUP_CONTAINMENT_STRUCTURED,
    DEDUP_IOU,
    DEDUP_MIN_CONTENT_CHARS,
    DEDUP_PLACEHOLDERS,
    DEDUP_TEXT_FUZZY_PREFIX,
    DEDUP_TEXT_MIN_OVERLAP,
    DEDUP_TEXT_RATIO,
    DEDUP_TEXT_TOKEN_CONTAINMENT,
    FIGURE_LINE_GAP_RATIO,
    FIGURE_LINE_MAX_HEIGHT,
    FIGURE_LINE_MAX_INK,
    FIGURE_LINE_MERGE_PX,
    FIGURE_LINE_MIN_HEIGHT,
    FIGURE_LINE_MIN_INK,
    FIGURE_LINE_MIN_WIDTH,
    FIGURE_LINE_MIN_X_OVERLAP,
    LAYOUT_MAX_LONG_SIDE,
    TILE_MIN_PAGE_HEIGHT,
    TILE_OVERLAP,
)
from app.services.boxes import (  # noqa: E402
    ORIGIN_MAIN,
    ORIGIN_TILE,
    BoxFilter,
    LayoutBox,
    looks_like_body_text,
    resolve_page_regions,
    split_tiles,
    texts_look_dual,
)
from app.services.textlines import detect_text_lines, group_lines, ink_mask  # noqa: E402


def box(rect, label="text", score=0.6, origin=ORIGIN_MAIN) -> LayoutBox:
    return LayoutBox(
        xyxy=np.array(rect, dtype=np.float32),
        label_id=0,
        label_name=label,
        score=score,
        origin=origin,
    )


def make_filter(**overrides) -> BoxFilter:
    args = dict(
        iou_threshold=DEDUP_IOU,
        containment_threshold=DEDUP_CONTAINMENT,
        structured_containment_threshold=DEDUP_CONTAINMENT_STRUCTURED,
        min_area=BOX_MIN_AREA,
        min_score=BOX_MIN_SCORE,
        expand_pixels=BOX_EXPAND_PIXELS,
        trim_margin=BOX_TRIM_MARGIN_PX,
        trim_min_area_ratio=BOX_TRIM_MIN_AREA_RATIO,
    )
    args.update(overrides)
    return BoxFilter(**args)


def resolve(items):
    return resolve_page_regions(
        items,
        min_overlap=DEDUP_TEXT_MIN_OVERLAP,
        ratio=DEDUP_TEXT_RATIO,
        token_containment=DEDUP_TEXT_TOKEN_CONTAINMENT,
        fuzzy_prefix=DEDUP_TEXT_FUZZY_PREFIX,
        min_content_chars=DEDUP_MIN_CONTENT_CHARS,
        placeholders=DEDUP_PLACEHOLDERS,
    ).regions


def resolve_page(items):
    return resolve_page_regions(
        items,
        min_overlap=DEDUP_TEXT_MIN_OVERLAP,
        ratio=DEDUP_TEXT_RATIO,
        token_containment=DEDUP_TEXT_TOKEN_CONTAINMENT,
        fuzzy_prefix=DEDUP_TEXT_FUZZY_PREFIX,
        min_content_chars=DEDUP_MIN_CONTENT_CHARS,
        placeholders=DEDUP_PLACEHOLDERS,
    )


def contains(outer: LayoutBox, inner: LayoutBox) -> bool:
    a, b = outer.xyxy, inner.xyxy
    return bool(a[0] <= b[0] + 0.01 and a[1] <= b[1] + 0.01 and a[2] >= b[2] - 0.01 and a[3] >= b[3] - 0.01)


def assert_lossless(test: unittest.TestCase, source: list[LayoutBox], kept: list[LayoutBox]) -> None:
    """Every rect that is not in the output must sit inside one that is — the merge's core promise."""
    kept_ids = {id(b) for b in kept}
    for dropped in (b for b in source if id(b) not in kept_ids):
        test.assertTrue(
            any(contains(survivor, dropped) for survivor in kept),
            f"dropped {dropped.xyxy.tolist()} is not covered by any survivor",
        )


class GeometryMergeTest(unittest.TestCase):
    """The pre-OCR merge: coverage of the smaller box is what decides, and nothing is lost."""

    def test_figure_caption_and_its_retold_last_line(self) -> None:
        # p2: "Figure 1: ... not hardware topology." plus a second box over just "topology." (0.54).
        caption = box([105, 507, 1067, 579], "figure_title", score=0.83)
        tail = box([101, 563, 340, 589], "figure_title", score=0.44, origin=ORIGIN_TILE)
        kept = make_filter().filter([caption, tail])
        self.assertEqual([b.label_name for b in kept], ["figure_title"])
        self.assertTrue(contains(kept[0], tail))
        assert_lossless(self, [caption, tail], kept)

    def test_author_line_and_affiliation_line(self) -> None:
        # p1: "Frank Li / UNSW Sydney · research@n1" over "UNSW Sydney · research@nla.net" (0.49).
        author = box([422, 213, 711, 270], score=0.71)
        affiliation = box([445, 255, 746, 282], score=0.66)
        kept = make_filter().filter([author, affiliation])
        self.assertEqual(len(kept), 1)
        self.assertTrue(contains(kept[0], affiliation))
        assert_lossless(self, [author, affiliation], kept)

    def test_cross_column_box_over_a_paragraph(self) -> None:
        # p2: a box straddling the gutter over the paragraph it re-detected (0.48).
        straddling = box([576, 903, 1021, 1000], score=0.52)
        paragraph = box([606, 950, 1076, 1052], score=0.69)
        kept = make_filter().filter([straddling, paragraph])
        self.assertEqual(len(kept), 1)
        self.assertTrue(contains(kept[0], straddling))

    def test_text_inside_a_table_follows_the_table(self) -> None:
        # p3: the table's own last row ("... injected save delay") also came back as a text box.
        table = box([106, 574, 1077, 904], "table", score=0.62)
        fragment = box([577, 881, 1027, 930], score=0.58)
        kept = make_filter().filter([table, fragment])
        self.assertEqual(len(kept), 1)
        self.assertEqual(kept[0].label_name, "table")

    def test_main_pass_wins_over_a_larger_recall_box(self) -> None:
        main = box([577, 1062, 1003, 1090], score=0.55)
        recall = box([560, 1050, 1050, 1105], score=0.79, origin=ORIGIN_TILE)
        kept = make_filter().filter([main, recall])
        self.assertEqual(len(kept), 1)
        self.assertEqual(kept[0].origin, ORIGIN_MAIN)
        assert_lossless(self, [main, recall], kept)

    def test_figure_does_not_swallow_the_title_above_it(self) -> None:
        # p3: the flow diagram's own top row sits mostly outside the figure's rect (0.30 coverage) —
        # it must stay a text block, or the reader loses a translated line.
        figure = box([115, 141, 1059, 433], "image", score=0.88)
        inner = box([120, 122, 682, 153], score=0.63)
        kept = make_filter().filter([figure, inner])
        self.assertEqual(sorted(b.label_name for b in kept), ["image", "text"])

    def test_junk_is_dropped_and_neighbours_survive(self) -> None:
        big = box([200, 200, 900, 400], score=0.7)
        tiny = box([205, 205, 215, 210], score=0.9)  # under min_area
        weak = box([1000, 1000, 1200, 1040], score=0.1)  # under min_score
        kept = make_filter().filter([big, tiny, weak])
        self.assertEqual([b.score for b in kept], [0.7])

    def test_kept_boxes_come_back_in_score_order(self) -> None:
        boxes = [box([100, 100, 300, 140], score=0.4), box([400, 100, 600, 140], score=0.9)]
        self.assertEqual([b.score for b in make_filter().filter(boxes)], [0.9, 0.4])


class TextMergeTest(unittest.TestCase):
    """Post-OCR merge: geometry raises the question, the text answers it."""

    def test_offset_heading_pair_keeps_the_complete_reading(self) -> None:
        # p2: "3 Introduction failures and repair" / "3 Integration failures and repair" (0.39).
        first = box([577, 1062, 1003, 1090], score=0.44)
        second = box([608, 1075, 968, 1107], score=0.51)
        kept = resolve([
            (first, "3 Introduction failures and repair"),
            (second, "3 Integration failures and repair"),
        ])
        self.assertEqual(len(kept), 1)
        self.assertIn("failures and repair", kept[0][1])

    def test_truncated_ocr_never_replaces_the_full_one(self) -> None:
        # p1: "... uses the final, exp" (cut short) against the complete sentence.
        truncated = box([576, 553, 1011, 603], score=0.77)
        complete = box([606, 582, 1063, 635], score=0.62)
        kept = resolve([
            (truncated, "treatment. The main evaluation uses the final, exp"),
            (complete, "treatment. The main evaluation uses the final, explicitly identified comparisons."),
        ])
        self.assertEqual(len(kept), 1)
        self.assertIn("explicitly identified", kept[0][1])
        self.assertTrue(contains(kept[0][0], truncated))

    def test_acknowledgements_detected_twice(self) -> None:
        # p8: the paragraph and a second box holding its heading plus its first line (0.43).
        paragraph = box([106, 848, 574, 925], score=0.69)
        heading = box([101, 806, 545, 880], score=0.57, origin=ORIGIN_TILE)
        kept = resolve([
            (paragraph, "The author acknowledges Research Technology Services, UNSW Sydney, for "
                        "providing GPU resources on the Katana computational cluster [7] for model "
                        "quantization."),
            (heading, "Acknowledgements\nThe author acknowledges Research Technology Service"),
        ])
        self.assertEqual(len(kept), 1)
        self.assertIn("Katana", kept[0][1])

    def test_fragment_of_the_abstract_is_absorbed(self) -> None:
        # p1: "equivalence, connota" — a box over the abstract's tail whose words all belong to it.
        abstract = box([106, 296, 1084, 563], "abstract", score=0.9)
        fragment = box([102, 551, 270, 578], score=0.42, origin=ORIGIN_TILE)
        kept = resolve([
            (abstract, "Abstract. External cache transfers can succeed while a hybrid language model "
                       "resumes from an inconsistent state, which makes the equivalence, connotation and "
                       "scope of the two positions the point of the comparison."),
            (fragment, "equivalence, connota"),
        ])
        self.assertEqual(len(kept), 1)
        self.assertEqual(kept[0][0].label_name, "abstract")

    def test_different_text_keeps_both_boxes(self) -> None:
        # Overlap alone is not a duplicate: two genuine neighbours must both survive.
        first = box([100, 100, 400, 160], score=0.7)
        second = box([120, 130, 420, 190], score=0.7)
        kept = resolve([
            (first, "Root cause analysis of the connector mismatch"),
            (second, "Experimental setup and numerical controls"),
        ])
        self.assertEqual(len(kept), 2)

    def test_repeated_short_label_in_two_places_survives(self) -> None:
        # The same words far apart are two cells/labels, not a duplicate.
        first = box([100, 100, 200, 130], score=0.7)
        second = box([100, 900, 200, 930], score=0.7)
        kept = resolve([(first, "Enabled"), (second, "Enabled")])
        self.assertEqual(len(kept), 2)

    def test_ghost_boxes_are_dropped_and_figures_are_not(self) -> None:
        items = [
            (box([100, 100, 300, 140]), "___"),
            (box([100, 200, 300, 240]), ""),
            (box([100, 300, 300, 340]), "A real paragraph of text."),
            (box([100, 400, 900, 700], "image"), ""),
            (box([100, 800, 900, 900], "table"), "<fcel>Setting<fcel>Value<nl>"),
        ]
        page = resolve_page_regions(
            items,
            min_overlap=DEDUP_TEXT_MIN_OVERLAP,
            ratio=DEDUP_TEXT_RATIO,
            token_containment=DEDUP_TEXT_TOKEN_CONTAINMENT,
        )
        self.assertEqual([b.label_name for b, _ in page.regions], ["text", "image", "table"])
        self.assertEqual([b.label_name for b in page.junk], ["text", "text"])

    def test_an_audit_of_what_was_merged_comes_back_with_the_page(self) -> None:
        items = [
            (box([100, 100, 400, 160], score=0.7),
             "Root cause analysis of the connector mismatch in detail."),
            (box([110, 115, 390, 165], score=0.6),
             "Root cause analysis of the connector mismatch"),
        ]
        page = resolve_page_regions(
            items,
            min_overlap=DEDUP_TEXT_MIN_OVERLAP,
            ratio=DEDUP_TEXT_RATIO,
            token_containment=DEDUP_TEXT_TOKEN_CONTAINMENT,
        )
        self.assertEqual(len(page.regions), 1)
        self.assertEqual([md for _, md in page.merged], ["Root cause analysis of the connector mismatch"])

    def test_the_page_accounting_adds_up(self) -> None:
        items = [
            (box([100, 100, 300, 140]), "___"),                                  # junk
            (box([100, 200, 400, 300], score=0.7), "The cache path stays cold."),
            (box([105, 205, 395, 305], score=0.6), "The cache path stays cold."),  # merged
            (box([100, 400, 900, 700], "image"), ""),                             # passthrough
            (box([500, 400, 900, 500]), "An unrelated paragraph on the same page."),
        ]
        page = resolve_page_regions(
            items,
            min_overlap=DEDUP_TEXT_MIN_OVERLAP,
            ratio=DEDUP_TEXT_RATIO,
            token_containment=DEDUP_TEXT_TOKEN_CONTAINMENT,
        )
        self.assertEqual(len(page.regions) + len(page.merged) + len(page.junk), len(items))

    def test_region_order_is_preserved(self) -> None:
        items = [
            (box([100, 100, 300, 140], "paragraph_title"), "2.1 Execution and cached state"),
            (box([100, 200, 900, 400], "table"), "<fcel>a<fcel>b<nl>"),
            (box([100, 500, 300, 560]), "Body text that stands on its own."),
            (box([105, 505, 305, 565]), "Body text that stands on its own."),
        ]
        self.assertEqual(
            [b.label_name for b, _ in resolve(items)], ["paragraph_title", "table", "text"],
        )

    def test_translation_columns_are_not_merged(self) -> None:
        # A CJK paragraph and its English original overlap on a bilingual page; the ratio must decide.
        kept = resolve([
            (box([100, 100, 400, 200]), "修复相对于在精确边界处的错误单令牌调度增加了重计算。"),
            (box([420, 100, 800, 200]), "The repair increases recomputation relative to the faulty "
                                        "one-token schedule at exact boundaries."),
        ])
        self.assertEqual(len(kept), 2)


class TextSimilarityTest(unittest.TestCase):
    def test_short_numeric_labels_are_not_duplicates(self) -> None:
        self.assertFalse(texts_look_dual("3", "31", ratio=0.82, token_containment=0.7))
        self.assertFalse(texts_look_dual("1", "1,792", ratio=0.82, token_containment=0.7))

    def test_identical_and_truncated_readings_are_duplicates(self) -> None:
        self.assertTrue(texts_look_dual("Enabled", "Enabled", ratio=0.82, token_containment=0.7))
        self.assertTrue(texts_look_dual("3 Integration failures and repair",
                                        "3 Introduction failures and repair",
                                        ratio=0.82, token_containment=0.7))
        self.assertTrue(texts_look_dual(
            "the cumulative patch alone preserves production outputs.",
            "it does not establish that the cumulative patch alone preserves production outputs.",
            ratio=0.82, token_containment=0.7,
        ))

    def test_cjk_with_a_stray_latin_term_is_not_duplicated_by_it(self) -> None:
        self.assertFalse(texts_look_dual(
            "这些检查回答了不同的问题。比较源和目标页面检查的是被审计的字节。",
            "修复相对于在精确边界处的错误单令牌调度增加了重计算。GLM 的配置保持不变。",
            ratio=0.82, token_containment=0.7,
        ))


class FigureGateTest(unittest.TestCase):
    """First half of the figure gate: what the VL read inside the figure decides whether it is text."""

    LIMITS = {"min_chars": 150, "min_lines": 3, "min_alnum_ratio": 0.6}

    def test_flowchart_labels_read_as_text(self) -> None:
        self.assertTrue(looks_like_body_text(
            "Prompt length N = 3,584; checkpoint interval C = 1,792\n"
            "Before\nRestore state\nB = 3,584\nScheduler credit\np = 3,583\n"
            "Compute 1 input token\nFirst difference: output index 11\n"
            "B != p: changing a count does not roll back loaded state",
            **self.LIMITS,
        ))

    def test_photo_yields_nothing(self) -> None:
        self.assertFalse(looks_like_body_text("", **self.LIMITS))
        self.assertFalse(looks_like_body_text("Figure\n", **self.LIMITS))

    def test_single_line_caption_stays_an_image(self) -> None:
        self.assertFalse(looks_like_body_text(
            "A photograph of the cluster rack with its four GPUs and the cooling loop visible.",
            **self.LIMITS,
        ))

    def test_symbol_heavy_axis_text_stays_an_image(self) -> None:
        self.assertFalse(looks_like_body_text(
            "| | | | | | | | | | |\n"
            "0 10 20 30 40 50 60 70 80 90 100\n"
            "---- ---- ---- ---- ---- ---- ---- ----\n"
            "|||||||||||||||||||||||||||||||||||||||||",
            **self.LIMITS,
        ))


class FigureLineScanTest(unittest.TestCase):
    """Second half of the figure gate: a diagram's labels are found, its furniture is not.

    The canvas mimics the measured case — a light backdrop panel drawn as a frame, a filled swatch, a
    rule, and labels of one and two lines — with the config's own line-scan parameters.
    """

    def canvas(self) -> Image.Image:
        image = Image.new("RGB", (600, 300), (255, 255, 255))
        draw = ImageDraw.Draw(image)
        # A backdrop panel drawn as an outline (measured ink 0.08-0.10 against 0.17-0.29 for text).
        draw.rectangle((20, 20, 580, 120), outline=(200, 200, 200), width=2)
        # A filled swatch (a photo/legend chip).
        draw.rectangle((480, 180, 570, 250), fill=(40, 40, 40))
        # A horizontal rule.
        draw.rectangle((30, 270, 400, 272), fill=(120, 120, 120))
        font = ImageFont.load_default(size=16)
        # One label per box, and one label on two lines.
        draw.text((40, 40), "Restore state", fill=(0, 0, 0), font=font)
        draw.text((40, 60), "B = 3,584", fill=(0, 0, 0), font=font)
        draw.text((200, 40), "Scheduler credit", fill=(0, 0, 0), font=font)
        draw.text((200, 62), "p = 3,583", fill=(0, 0, 0), font=font)
        draw.text((40, 160), "Before", fill=(0, 0, 0), font=font)
        draw.text((40, 200), "After", fill=(0, 0, 0), font=font)
        return image

    def scan(self, image: Image.Image):
        return detect_text_lines(
            image,
            merge_px=FIGURE_LINE_MERGE_PX,
            min_height=FIGURE_LINE_MIN_HEIGHT,
            max_height=FIGURE_LINE_MAX_HEIGHT,
            min_width=FIGURE_LINE_MIN_WIDTH,
            min_ink=FIGURE_LINE_MIN_INK,
            max_ink=FIGURE_LINE_MAX_INK,
        )

    def test_text_is_found_and_furniture_is_not(self) -> None:
        lines = self.scan(self.canvas())
        self.assertGreaterEqual(len(lines), 6)
        for x1, y1, x2, y2 in lines:
            # Nothing that came from the backdrop frame, the swatch or the rule.
            self.assertFalse(y1 >= 175 and x1 >= 475, f"swatch kept: {(x1, y1, x2, y2)}")
            self.assertFalse(y1 >= 265, f"rule kept: {(x1, y1, x2, y2)}")
            self.assertGreater(y2, y1)
            self.assertGreater(x2, x1)

    def test_lines_of_one_label_group_but_columns_stay_apart(self) -> None:
        blocks = group_lines(
            self.scan(self.canvas()),
            gap_ratio=FIGURE_LINE_GAP_RATIO,
            min_x_overlap=FIGURE_LINE_MIN_X_OVERLAP,
        )
        self.assertGreaterEqual(len(blocks), 4)
        # "Restore state" + "B = 3,584" is one block; "Scheduler credit" is another.
        heights = sorted((y2 - y1) for _, y1, _, y2 in blocks)
        self.assertGreater(heights[-1], 2 * heights[0])

    def test_a_blank_figure_yields_nothing(self) -> None:
        self.assertEqual(self.scan(Image.new("RGB", (400, 300), (255, 255, 255))), [])


class InkTrimTest(unittest.TestCase):
    """The detector's boxes carry a margin; the trim is what stops neighbours overlapping."""

    def mask(self) -> np.ndarray:
        ink = np.zeros((400, 600), dtype=bool)
        ink[100:140, 100:500] = True  # one line of text
        return ink

    def test_a_box_is_pulled_tight_onto_its_ink(self) -> None:
        # The rect ends up the ink's own bbox plus BOX_TRIM_MARGIN_PX a side: the detector's slack goes,
        # a normal margin (the detector's own median is 5-9 px) stays so covers are not cramped.
        margin = int(BOX_TRIM_MARGIN_PX)
        kept = make_filter(trim_margin=BOX_TRIM_MARGIN_PX).filter(
            [box([60, 60, 560, 200], score=0.9)], page_size=(600, 400), ink=self.mask(),
        )
        self.assertEqual(len(kept), 1)
        self.assertEqual(
            kept[0].int_rect, (100 - margin, 100 - margin, 500 + margin, 140 + margin)
        )

    def test_a_box_with_almost_no_ink_inside_keeps_its_rect(self) -> None:
        # A speck in a 300x200 box: the measurement is not trustworthy, so nothing moves.
        ink = np.zeros((400, 600), dtype=bool)
        ink[150:152, 300:302] = True
        kept = make_filter(trim_margin=BOX_TRIM_MARGIN_PX).filter(
            [box([100, 100, 400, 300], score=0.9)], page_size=(600, 400), ink=ink,
        )
        self.assertEqual(len(kept), 1)
        self.assertEqual(kept[0].int_rect, (98, 98, 402, 302))

    def test_trimming_reveals_a_duplicate_the_margin_was_hiding(self) -> None:
        # Two detections of the same line, one with a wide margin that used to keep them apart.
        ink = np.zeros((400, 600), dtype=bool)
        ink[100:140, 100:500] = True
        tight = box([100, 100, 500, 140], score=0.8)
        padded = box([60, 60, 560, 200], score=0.9)
        kept = make_filter().filter([tight, padded], page_size=(600, 400), ink=ink)
        self.assertEqual(len(kept), 1)
        # The margin is empty by definition, so the invariant is about what the boxes cover, not
        # about their generous outlines.
        content = make_filter().filter([padded], page_size=(600, 400), ink=ink)[0]
        assert_lossless(self, [tight, content], kept)

    def test_a_box_with_only_a_sliver_of_ink_keeps_its_rect(self) -> None:
        # The measured spurious case: a 344x31 detection whose whole ink is a 2 px sliver. Shrinking
        # onto it would cut whatever the ink mask failed to see, so the rect stays — the reading is
        # what disqualifies such a box (one character), not its geometry.
        ink = np.zeros((400, 600), dtype=bool)
        ink[320:323, 288:290] = True
        kept = make_filter(trim_margin=BOX_TRIM_MARGIN_PX).filter(
            [box([288, 320, 632, 351], score=0.6)], page_size=(600, 400), ink=ink,
        )
        self.assertEqual(len(kept), 1)
        self.assertEqual(kept[0].int_rect, (286, 318, 600, 353))

    def test_trimming_is_off_when_the_margin_is_negative(self) -> None:
        kept = make_filter(trim_margin=-1.0).filter(
            [box([60, 60, 560, 200], score=0.9)], page_size=(600, 400), ink=self.mask(),
        )
        self.assertEqual(kept[0].int_rect, (58, 58, 562, 202))


class ContentQualityTest(unittest.TestCase):
    """What a text-family box has to read as before it is treated as content."""

    def test_a_stray_glyph_is_not_content(self) -> None:
        items = [
            (box([100, 100, 300, 140], score=0.7), "I"),
            (box([100, 200, 300, 240], score=0.7), "1"),
            (box([100, 300, 300, 340], score=0.7), "R3"),
        ]
        self.assertEqual([md for _, md in resolve(items)], ["R3"])

    def test_the_vl_placeholder_is_not_content(self) -> None:
        items = [
            (box([100, 100, 300, 140], score=0.7), "[Unlabeled]"),
            (box([100, 200, 300, 240], score=0.7), "A real caption."),
        ]
        self.assertEqual([md for _, md in resolve(items)], ["A real caption."])

    def test_a_truncated_word_still_matches(self) -> None:
        self.assertTrue(texts_look_dual(
            "equivalence, connota",
            "Abstract. External cache transfers can succeed while a hybrid language model resumes "
            "from an inconsistent state, which makes the equivalence, connotation and scope of the "
            "two positions the point of the comparison.",
            ratio=DEDUP_TEXT_RATIO, token_containment=DEDUP_TEXT_TOKEN_CONTAINMENT,
            fuzzy_prefix=DEDUP_TEXT_FUZZY_PREFIX,
        ))

    def test_a_truncated_word_does_not_match_a_different_one(self) -> None:
        self.assertFalse(texts_look_dual(
            "integration of the cache path",
            "introduction to the cache path layout",
            ratio=DEDUP_TEXT_RATIO, token_containment=DEDUP_TEXT_TOKEN_CONTAINMENT,
            fuzzy_prefix=DEDUP_TEXT_FUZZY_PREFIX,
        ))

    def test_the_fragment_of_a_paragraph_is_absorbed(self) -> None:
        # The real pair: an 18-character fragment box overlapping the abstract's last line, whose
        # reading is a truncation of a word in the abstract.
        abstract = box([106, 296, 1084, 563], "abstract", score=0.9)
        fragment = box([102, 551, 270, 578], score=0.42, origin=ORIGIN_TILE)
        kept = resolve([
            (abstract, "Abstract. External cache transfers can succeed while a hybrid language model "
                       "resumes from an inconsistent state, which makes the equivalence, connotation "
                       "and scope of the two positions the point of the comparison."),
            (fragment, "equivalence, connota"),
        ])
        self.assertEqual(len(kept), 1)
        self.assertEqual(kept[0][0].label_name, "abstract")


class TileGeometryTest(unittest.TestCase):
    """Tile rects are page coordinates, which is what keeps the tile pass' boxes on their text.

    The pass feeds the detector a crop, so the crop rect, the target size it asks the boxes back in
    and the shift it applies to them have to be one space. Handing the detector a *downscaled* copy of
    the page while shifting in the page's own pixels (an A4 render is 1684 px tall, over
    ``LAYOUT_MAX_LONG_SIDE``) displaced every tile box by the downscale factor: 0.4 mean IoU against
    the same page's main pass, 0.92 once rescaled, and up to 42 pt of drift at the bottom of the page.
    """

    def test_the_tiles_cover_the_page_and_keep_the_overlap(self) -> None:
        page = (1191, 1684)
        top, bottom = split_tiles(*page, TILE_OVERLAP, TILE_MIN_PAGE_HEIGHT)
        overlap = int(page[1] * TILE_OVERLAP)
        self.assertEqual(top, (0, 0, page[0], page[1] // 2 + overlap))
        self.assertEqual(bottom, (0, page[1] // 2 - overlap, page[0], page[1]))
        # every row of the page is in at least one tile, and neither tile leaves the page
        self.assertGreaterEqual(top[3], bottom[1])
        self.assertEqual(bottom[3], page[1])

    def test_the_tile_offset_is_a_page_coordinate(self) -> None:
        # Regression: the tiles were cut from the page downscaled to LAYOUT_MAX_LONG_SIDE while their
        # offsets stayed in page pixels, so a tile box landed up and left of its text by the downscale
        # factor (up to 42 pt here) — the reader sees that as a cover over the wrong lines. The offset
        # is the rect's own origin, in the page's grid.
        page = (1191, 1684)  # scale-2.0 A4, what the frontend renders
        bottom = split_tiles(*page, TILE_OVERLAP, TILE_MIN_PAGE_HEIGHT)[1]
        self.assertEqual(bottom[1], page[1] // 2 - int(page[1] * TILE_OVERLAP))
        # and the cap has to stay above that render, or every page is downscaled (and resampled twice,
        # since the layout processor resizes to 800x800 of its own)
        self.assertGreater(LAYOUT_MAX_LONG_SIDE, page[1])

    def test_a_short_page_is_not_tiled(self) -> None:
        self.assertEqual(
            split_tiles(400, TILE_MIN_PAGE_HEIGHT - 1, TILE_OVERLAP, TILE_MIN_PAGE_HEIGHT), []
        )


if __name__ == "__main__":
    unittest.main(verbosity=2)
