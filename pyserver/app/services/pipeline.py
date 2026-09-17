"""OCR pipeline (ported from Wise-Paddle core_pipeline.py; concurrency and disk output removed).

Per page: PP-DocLayoutV3 layout (three add-only passes) → BoxFilter (threshold + merge) → numpy crop →
PaddleOCR-VL-1.6 batched per label → post-OCR text dedup → in-memory RegionResult (never written to
disk). Rust is the sole scheduler and results return over HTTP. Text-dense figures get a fourth pass
that lifts their inner text out as ordinary blocks (see _figure_regions). Every number lives in
app/config.py; the geometry and text-dedup primitives live in boxes.py.
"""

from __future__ import annotations

import logging
import time
from typing import Optional, Sequence

import numpy as np
import torch
from PIL import Image, ImageFilter
from transformers import (
    AutoImageProcessor,
    AutoModelForObjectDetection,
    AutoModelForImageTextToText,
    AutoProcessor,
)

from .boxes import (
    FIGURE_LABELS,
    ORIGIN_FALLBACK,
    ORIGIN_FIGURE,
    ORIGIN_MAIN,
    ORIGIN_TILE,
    TEXT_LABELS,
    BoxFilter,
    LayoutBox,
    PageRegions,
    PageResult,
    RegionResult,
    containment,
    iou_xyxy,
    looks_like_body_text,
    resolve_page_regions,
    split_tiles,
)
from .textlines import detect_text_lines, group_lines, ink_mask

logger = logging.getLogger("ezpdf.pipeline")


# Restore the "default" RoPE init entry that transformers 5.x dropped (older configs reference it)
def _patch_rope_default() -> None:
    """Re-register an equivalent ``rope_type="default"`` implementation so older configs load unedited."""
    import transformers.modeling_rope_utils as rope_utils

    if "default" in rope_utils.ROPE_INIT_FUNCTIONS:
        return

    def _compute_default_rope_parameters(
            config,
            device=None,
            seq_len=None,
            layer_type=None,
    ):
        base = config.rope_theta
        dim = getattr(config, "head_dim", None) or (
                config.hidden_size // config.num_attention_heads
        )
        inv_freq = 1.0 / (
                base
                ** (
                        torch.arange(0, dim, 2, dtype=torch.int64).float().to(device)
                        / dim
                )
        )
        return inv_freq, 1.0

    rope_utils.ROPE_INIT_FUNCTIONS["default"] = _compute_default_rope_parameters
    logger.info("Patched transformers ROPE_INIT_FUNCTIONS['default']")


# Module-level; must stay for older model configs to load (idempotent, safe to call repeatedly).
_patch_rope_default()


class LayoutDetector:
    def __init__(
            self,
            model_path: str,
            device: torch.device,
            score_threshold: float,
            max_long_side: int,
            fallback_threshold: float,
            fallback_sharpen: tuple[int, int, int],
            fallback_iou: float,
            fallback_containment: float,
            tile_threshold: float,
            tile_overlap: float,
            tile_iou: float,
            tile_containment: float,
            tile_min_page_height: int,
            forward_batch: int,
            dtype: torch.dtype = torch.float32,
    ) -> None:
        logger.info("Loading layout model: %s", model_path)
        self.processor = AutoImageProcessor.from_pretrained(model_path)
        self.model = (
            AutoModelForObjectDetection.from_pretrained(model_path, dtype=dtype)
            .to(device)
            .eval()
        )
        self.device = device
        self.score_threshold = score_threshold
        self.id2label: dict[int, str] = {
            int(k): v for k, v in self.model.config.id2label.items()
        }
        # The processor has a built-in 800x800 resize and adapts as long as the long side is sane
        self._max_long_side = int(max_long_side)  # cap so 4K+ images cannot blow up VRAM
        # Second-pass recall whitelist (see detect): text family only, since a lower threshold on a
        # sharpened image also surfaces stickers/seals that are not text
        self._fallback_labels = frozenset({
            "text", "paragraph_title", "doc_title", "abstract", "aside_text",
            "footnote", "figure_title", "content", "reference", "reference_content",
        })
        self._fallback_threshold = fallback_threshold
        self._fallback_sharpen = fallback_sharpen  # PIL UnsharpMask: radius, percent, threshold
        self._fallback_iou = fallback_iou
        self._fallback_containment = fallback_containment
        # Third pass, half-page tiles (see detect); add-only, and its whitelist also takes header/footer
        # because a cover's standard line is classified as header
        self._tile_labels = self._fallback_labels | {"header", "footer"}
        self._tile_threshold = tile_threshold
        self._tile_overlap = tile_overlap
        self._tile_iou = tile_iou
        self._tile_containment = tile_containment
        self._tile_min_page_height = int(tile_min_page_height)
        # batch=8 hits a slow kernel (measured 6.0s vs 0.22s at batch=4), so all forwards are chunked
        self._max_forward_batch = max(1, int(forward_batch))

    def _maybe_downscale(self, image: Image.Image) -> Image.Image:
        w, h = image.size
        m = max(w, h)
        if m <= self._max_long_side:
            return image
        scale = self._max_long_side / m
        return image.resize(
            (max(1, int(w * scale)), max(1, int(h * scale))),
            Image.Resampling.BILINEAR,
        )

    @torch.no_grad()
    def _forward(
            self,
            images: Sequence[Image.Image],
            target_sizes: torch.Tensor,
            threshold: float,
            origin: int,
    ) -> list[list[LayoutBox]]:
        """Chunked forward (≤ ``_max_forward_batch``); the slow batch=8 kernel is why, see _max_forward_batch."""
        images = list(images)
        if len(images) <= self._max_forward_batch:
            return self._forward_one(images, target_sizes, threshold, origin)
        out: list[list[LayoutBox]] = []
        for i in range(0, len(images), self._max_forward_batch):
            chunk = images[i:i + self._max_forward_batch]
            out.extend(
                self._forward_one(chunk, target_sizes[i:i + len(chunk)], threshold, origin)
            )
        return out

    @torch.no_grad()
    def _forward_one(
            self,
            images: Sequence[Image.Image],
            target_sizes: torch.Tensor,
            threshold: float,
            origin: int,
    ) -> list[list[LayoutBox]]:
        inputs = self.processor(images=list(images), return_tensors="pt").to(self.device)
        outputs = self.model(**inputs)
        # post_process does a first coarse cut at threshold; BoxFilter filters IoU/area finer afterwards
        raw = self.processor.post_process_object_detection(
            outputs, target_sizes=target_sizes, threshold=threshold
        )

        # post_process returns boxes in target_sizes coordinates, which is NOT the size of the image
        # handed to the model when it was downscaled for VRAM — clamping to the input would shave the
        # last ~30pt of every page (crops lose text; a footer can vanish entirely).
        sizes = target_sizes.detach().cpu().tolist()

        out: list[list[LayoutBox]] = []
        for r, (height, width) in zip(raw, sizes):
            boxes = r["boxes"].detach().cpu().numpy()
            labels = r["labels"].detach().cpu().numpy()
            scores = r["scores"].detach().cpu().numpy()
            page_boxes: list[LayoutBox] = []
            for box, lid, sc in zip(boxes, labels, scores):
                x1, y1, x2, y2 = box
                x1 = max(0.0, min(float(x1), float(width)))
                x2 = max(0.0, min(float(x2), float(width)))
                y1 = max(0.0, min(float(y1), float(height)))
                y2 = max(0.0, min(float(y2), float(height)))
                page_boxes.append(
                    LayoutBox(
                        xyxy=np.array([x1, y1, x2, y2], dtype=np.float32),
                        label_id=int(lid),
                        label_name=self.id2label.get(int(lid), str(int(lid))),
                        score=float(sc),
                        origin=origin,
                    )
                )
            out.append(page_boxes)
        return out

    def _append_candidate(
            self,
            page_boxes: list[LayoutBox],
            candidate: LayoutBox,
            labels: frozenset[str],
            iou_limit: float,
            containment_limit: float,
    ) -> bool:
        """Add-only merge: whitelisted label and low overlap with existing/added boxes (both IoU and
        containment). True when the candidate was kept — the caller counts what each pass contributed."""
        if candidate.label_name not in labels:
            return False
        for k in page_boxes:
            if iou_xyxy(candidate.xyxy, k.xyxy) > iou_limit:
                return False
            # A figure never absorbs the text inside it: that text is what the figure pass (and the
            # reader's cover) wants, and rejecting it here would make it unreachable for every pass.
            if k.label_name in FIGURE_LABELS and candidate.label_name not in FIGURE_LABELS:
                continue
            if containment(candidate.xyxy, k.xyxy) > containment_limit:
                return False
        page_boxes.append(candidate)
        return True

    @torch.no_grad()
    def detect(self, images: Sequence[Image.Image]) -> list[list[LayoutBox]]:
        """Run layout detection on a batch of images (multi-pass, add-only).

        pdfjs-rendered text scores ~0.1-0.15 below pdfium, so whole blocks go missing; three add-only
        passes (results are never replaced, since sharpening lowers some edge scores): source at 0.5,
        sharpened full image at 0.45 (text-family), sharpened half-page tiles at 0.38.
        Candidates only pass with low overlap vs kept boxes (IoU/containment); whitelists in __init__.
        Boxes keep the pass they came from (``origin``) so the merge can prefer the main pass.

        Every pass reports in the caller's page pixels: the main and fallback passes feed the detector
        the possibly-downscaled image but ask for the page's size back, and the tile pass crops the
        page itself before downscaling the crop (boxes.split_tiles).
        """
        if not images:
            return []
        pages = [im.convert("RGB") for im in images]
        scaled = [self._maybe_downscale(im) for im in pages]

        target_sizes = torch.tensor(
            [[im.height, im.width] for im in pages], device=self.device
        )
        out = self._forward(scaled, target_sizes, self.score_threshold, ORIGIN_MAIN)

        fallback = [
            im.filter(ImageFilter.UnsharpMask(*self._fallback_sharpen)) for im in scaled
        ]
        main_counts = [len(page_boxes) for page_boxes in out]
        fallback_added = [0] * len(out)
        for index, (page_boxes, candidates) in enumerate(
            zip(out, self._forward(fallback, target_sizes, self._fallback_threshold, ORIGIN_FALLBACK))
        ):
            for b in candidates:
                fallback_added[index] += self._append_candidate(
                    page_boxes, b, self._fallback_labels,
                    self._fallback_iou, self._fallback_containment,
                )

        # Tiles come off the page-sized images, never off ``scaled``: their crop rect, the target size
        # handed to the detector and the shift applied to the boxes back must be one space, and the
        # page's own pixels are that space (see boxes.split_tiles).
        tiles: list[Image.Image] = []
        tile_sizes: list[list[int]] = []
        tile_rects_per_page: list[list[tuple[int, int, int, int]]] = []
        for im in pages:
            rects = split_tiles(im.width, im.height, self._tile_overlap, self._tile_min_page_height)
            tile_rects_per_page.append(rects)
            for x1, y1, x2, y2 in rects:
                tiles.append(self._maybe_downscale(im.crop((x1, y1, x2, y2))))
                tile_sizes.append([y2 - y1, x2 - x1])
        tile_added = [0] * len(out)
        if tiles:
            raw_tiles = self._forward(
                [t.filter(ImageFilter.UnsharpMask(*self._fallback_sharpen)) for t in tiles],
                torch.tensor(tile_sizes, device=self.device),
                self._tile_threshold, ORIGIN_TILE,
            )
            cursor = 0
            for index, (page_boxes, rects) in enumerate(zip(out, tile_rects_per_page)):
                for x1, y1, _, _ in rects:
                    shift = np.array([x1, y1, x1, y1], dtype=np.float32)
                    for b in raw_tiles[cursor]:
                        tile_added[index] += self._append_candidate(
                            page_boxes,
                            LayoutBox(
                                xyxy=b.xyxy + shift,
                                label_id=b.label_id,
                                label_name=b.label_name,
                                score=b.score,
                                origin=ORIGIN_TILE,
                            ),
                            self._tile_labels, self._tile_iou, self._tile_containment,
                        )
                    cursor += 1
        # How much each pass contributed is the only way to tell whether a recall pass still earns its
        # forwards on a given kind of page: both are add-only, so a page where they add nothing looks
        # exactly like a page they never ran on.
        for index, (fallback_count, tile_count) in enumerate(zip(fallback_added, tile_added)):
            logger.info(
                "  [layout] page %d: main=%d fallback=+%d tile=+%d",
                index + 1, main_counts[index], fallback_count, tile_count,
            )
        return out


class RegionCropper:
    def crop(self, rgb: np.ndarray, box: LayoutBox) -> np.ndarray:
        """Crop ``box`` from RGB ``(H,W,3)`` uint8; a collapsed box yields a 1×1 black placeholder."""
        x1, y1, x2, y2 = box.int_rect
        h, w = rgb.shape[:2]
        x1 = max(0, min(x1, w))
        x2 = max(0, min(x2, w))
        y1 = max(0, min(y1, h))
        y2 = max(0, min(y2, h))
        if x2 <= x1 or y2 <= y1:
            return np.zeros((1, 1, 3), dtype=np.uint8)
        return rgb[y1:y2, x1:x2].copy()


class VLPredictor:
    # Official PaddleOCR-VL task prompts; unlisted labels fall back to "_default" (_prompt_for).
    DEFAULT_PROMPTS: dict[str, str] = {
        "table": "Table Recognition:",
        "formula": "Formula Recognition:",
        "chart": "Chart Recognition:",
        "image": "OCR:",
        "_default": "OCR:",
    }

    def __init__(
            self,
            model_path: str,
            device: torch.device,
            prompts: Optional[dict[str, str]] = None,
            dtype: torch.dtype = torch.bfloat16,
            max_new_tokens: int = 512,
            max_pixels: int = 1280 * 28 * 28,
            min_pixels: int = 112896,
            max_forward_batch: int = 10,  # max images per VL forward (VRAM cap)
            attn_impl: str = "sdpa",
            repetition_penalty: float = 1.15,  # prevents repetition loops in batched inference
            do_sample: bool = False,  # greedy decode; set True for more variety
    ) -> None:
        logger.info("Loading VL model: %s", model_path)
        self.processor = AutoProcessor.from_pretrained(model_path)
        # Critical: decoder-only requires left padding, otherwise batched inference is scrambled
        try:
            self.processor.tokenizer.padding_side = "left"
        except Exception:
            pass
        # Critical: must be AutoModelForImageTextToText, not AutoModelForCausalLM (that entry point
        # left the model blind to images and it invented output)
        self.model = (
            AutoModelForImageTextToText.from_pretrained(
                model_path,
                dtype=dtype,
                attn_implementation=attn_impl,
            )
            .to(device)
            .eval()
        )
        self.device = device
        self.dtype = dtype
        self.max_new_tokens = int(max_new_tokens)
        self.max_pixels = int(max_pixels)
        self.min_pixels = int(min_pixels)
        self.max_forward_batch = max(1, int(max_forward_batch))
        self.repetition_penalty = float(repetition_penalty)
        self.do_sample = bool(do_sample)
        self.prompts = {**self.DEFAULT_PROMPTS, **(prompts or {})}
        # EOS: the model's generation_config.json uses </s> (id=2)
        tok = self.processor.tokenizer
        self.eos_token_id = tok.convert_tokens_to_ids("</s>") or tok.eos_token_id or 2
        self.pad_token_id = (
                self.processor.tokenizer.pad_token_id
                or tok.convert_tokens_to_ids("<unk>")
                or 0
        )

    def _prompt_for(self, label: str) -> str:
        return self.prompts.get(label, self.prompts["_default"])

    def _build_messages(
            self, images: Sequence[Image.Image], label: str
    ) -> list[list[dict]]:
        user_text = self._prompt_for(label)
        return [
            [
                {
                    "role": "user",
                    "content": [
                        {"type": "image", "image": img},
                        {"type": "text", "text": user_text},
                    ],
                }
            ]
            for img in images
        ]

    def _forward_once(self, images: Sequence[Image.Image], label: str) -> list[str]:
        """One VL forward; the caller must ensure ``len(images) <= self.max_forward_batch``."""
        if not images:
            return []
        conversations = self._build_messages(images, label)
        inputs = self.processor.apply_chat_template(
            conversations,
            add_generation_prompt=True,
            tokenize=True,
            return_dict=True,
            return_tensors="pt",
            # transformers 5.x: processor kwargs must live in ``processor_kwargs`` else a warning fires
            processor_kwargs={
                "padding": True,  # padding required for batched inference
                "images_kwargs": {
                    "size": {
                        "shortest_edge": self.min_pixels,
                        "longest_edge": self.max_pixels,
                    }
                },
            },
        ).to(self.device)
        ids = inputs["input_ids"]
        # Image tokens are marked via mm_token_type_ids (apply_chat_template does not insert <|image_pad|>)
        cnt = 0
        if "mm_token_type_ids" in inputs:
            cnt = int((inputs["mm_token_type_ids"] == 1).sum().item())
        logger.info(
            "  [VL fwd=%d label=%s] seq_len=%d image_tokens=%d",
            len(images), label, ids.shape[1], cnt,
        )
        out = self.model.generate(
            **inputs,
            max_new_tokens=self.max_new_tokens,
            do_sample=self.do_sample,
            eos_token_id=self.eos_token_id,
            pad_token_id=self.pad_token_id,
            use_cache=True,
            repetition_penalty=self.repetition_penalty,
        )
        gen = out[:, inputs["input_ids"].shape[1]:]
        return self.processor.batch_decode(gen, skip_special_tokens=True)

    @torch.no_grad()
    def recognize_batch(
            self, images: Sequence[Image.Image], label: str
    ) -> list[str]:
        """Batch inference with one prompt per label; splits into sub-batches above max_forward_batch."""
        if not images:
            return []
        cap = self.max_forward_batch
        if len(images) <= cap:
            return self._forward_once(images, label)
        out: list[str] = []
        for start in range(0, len(images), cap):
            chunk = list(images[start:start + cap])
            out.extend(self._forward_once(chunk, label))
        return out

    @torch.no_grad()
    def recognize_grouped(
            self, items: list[tuple[Image.Image, str]]
    ) -> list[str]:
        """Bucket by label → one batch inference per bucket → restore the original order."""
        buckets: dict[str, list[tuple[int, Image.Image]]] = {}
        for idx, (img, lab) in enumerate(items):
            buckets.setdefault(lab, []).append((idx, img))

        results: list[Optional[str]] = [None] * len(items)
        for label, bucket in buckets.items():
            indices = [i for i, _ in bucket]
            imgs = [im for _, im in bucket]
            texts = self.recognize_batch(imgs, label)
            for i, t in zip(indices, texts):
                results[i] = t
        return [r or "" for r in results]  # type: ignore[arg-type]


class OCRPipeline:
    """Run layout detection → filter → crop → VL recognition → text dedup → in-memory results.

    Every number is a constructor argument sourced from app/config.py (engine.py passes ocr_tuning());
    there are no defaults here on purpose, so the tuning has exactly one home.
    """

    def __init__(
            self,
            layout_model_path: str,
            vl_model_path: str,
            device: torch.device,
            *,
            score_threshold: float,
            max_long_side: int,
            fallback_threshold: float,
            fallback_sharpen: tuple[int, int, int],
            fallback_iou: float,
            fallback_containment: float,
            tile_threshold: float,
            tile_overlap: float,
            tile_iou: float,
            tile_containment: float,
            tile_min_page_height: int,
            layout_forward_batch: int,
            box_iou_threshold: float,
            box_containment_threshold: float,
            box_structured_containment_threshold: float,
            box_trim_to_ink: bool,
            box_trim_margin: float,
            box_trim_min_area_ratio: float,
            box_min_area: float,
            box_min_score: float,
            box_expand_pixels: float,
            max_regions: int,
            dedup_text_min_overlap: float,
            dedup_text_ratio: float,
            dedup_text_token_containment: float,
            dedup_text_fuzzy_prefix: int,
            dedup_min_content_chars: int,
            dedup_placeholders: tuple[str, ...],
            dedup_text_max_loss: float,
            figure_text_min_chars: int,
            figure_text_min_lines: int,
            figure_text_min_alnum_ratio: float,
            figure_line_merge_px: int,
            figure_line_min_height: int,
            figure_line_max_height: int,
            figure_line_min_width: int,
            figure_line_min_ink: float,
            figure_line_max_ink: float,
            figure_line_gap_ratio: float,
            figure_line_min_x_overlap: float,
            figure_inner_min_boxes: int,
            figure_inner_min_coverage: float,
            figure_min_side_px: int,
            max_new_tokens: int,
            vl_min_pixels: int,
            vl_max_pixels: int,
            vl_max_forward_batch: int,
            vl_repetition_penalty: float,
            box_unclip_ratio: float,
            vl_do_sample: bool = False,
            dtype: torch.dtype = torch.bfloat16,
            attn_impl: str = "sdpa",
    ) -> None:
        self.device = device
        self.layout = LayoutDetector(
            layout_model_path, device,
            score_threshold=score_threshold,
            max_long_side=max_long_side,
            fallback_threshold=fallback_threshold,
            fallback_sharpen=fallback_sharpen,
            fallback_iou=fallback_iou,
            fallback_containment=fallback_containment,
            tile_threshold=tile_threshold,
            tile_overlap=tile_overlap,
            tile_iou=tile_iou,
            tile_containment=tile_containment,
            tile_min_page_height=tile_min_page_height,
            forward_batch=layout_forward_batch,
        )
        # NOTE: not self.filter — would shadow the builtin filter()
        self.box_filter = BoxFilter(
            iou_threshold=box_iou_threshold,
            containment_threshold=box_containment_threshold,
            structured_containment_threshold=box_structured_containment_threshold,
            min_area=box_min_area,
            min_score=box_min_score,
            expand_pixels=box_expand_pixels,
            trim_margin=box_trim_margin if box_trim_to_ink else -1.0,
            trim_min_area_ratio=box_trim_min_area_ratio,
            unclip_ratio=box_unclip_ratio,
        )
        self.cropper = RegionCropper()
        self.vl = VLPredictor(
            vl_model_path, device,
            max_new_tokens=max_new_tokens,
            min_pixels=vl_min_pixels,
            max_pixels=vl_max_pixels,
            max_forward_batch=vl_max_forward_batch,
            repetition_penalty=vl_repetition_penalty,
            do_sample=vl_do_sample,
            attn_impl=attn_impl,
            dtype=dtype,
        )
        self.max_regions = int(max_regions)
        self.figure_min_side = int(figure_min_side_px)
        self.figure_text_min_chars = int(figure_text_min_chars)
        self.figure_text_min_lines = int(figure_text_min_lines)
        self.figure_text_min_alnum_ratio = float(figure_text_min_alnum_ratio)
        self.figure_line_merge_px = int(figure_line_merge_px)
        self.figure_line_min_height = int(figure_line_min_height)
        self.figure_line_max_height = int(figure_line_max_height)
        self.figure_line_min_width = int(figure_line_min_width)
        self.figure_line_min_ink = float(figure_line_min_ink)
        self.figure_line_max_ink = float(figure_line_max_ink)
        self.figure_line_gap_ratio = float(figure_line_gap_ratio)
        self.figure_line_min_x_overlap = float(figure_line_min_x_overlap)
        self.figure_inner_min_boxes = int(figure_inner_min_boxes)
        self.figure_inner_min_coverage = float(figure_inner_min_coverage)
        self.dedup_text_min_overlap = float(dedup_text_min_overlap)
        self.dedup_text_ratio = float(dedup_text_ratio)
        self.dedup_text_token_containment = float(dedup_text_token_containment)
        self.dedup_text_fuzzy_prefix = int(dedup_text_fuzzy_prefix)
        self.dedup_min_content_chars = int(dedup_min_content_chars)
        self.dedup_placeholders = tuple(dedup_placeholders)
        self.dedup_text_max_loss = float(dedup_text_max_loss)

    def _crop_page(
            self,
            image: Image.Image,
            layouts: list[LayoutBox],
            ink: Optional[np.ndarray] = None,
    ) -> list[tuple[Image.Image, LayoutBox]]:
        kept = self.box_filter.filter(layouts, page_size=(image.width, image.height), ink=ink)
        if len(kept) > self.max_regions:
            # Silent before, and the score order means the 0.38-0.45 recall candidates are the ones
            # dropped — the tuning knob is the way out, this line is how you find out you need it.
            logger.warning(
                "  [regions] page has %d boxes, keeping the top %d by score",
                len(kept), self.max_regions,
            )
            kept = kept[: self.max_regions]
        crops: list[tuple[Image.Image, LayoutBox]] = []
        # one RGB conversion per page
        rgb = np.asarray(image.convert("RGB"))
        for box in kept:
            crop = self._crop(rgb, box)
            if crop is not None:
                crops.append((crop, box))
        return crops

    def _crop(self, rgb: np.ndarray, box: LayoutBox) -> Optional[Image.Image]:
        arr = self.cropper.crop(rgb, box)
        if arr.size == 0 or arr.shape[0] < 2 or arr.shape[1] < 2:
            return None
        return Image.fromarray(arr)

    def _figure_markdown_is_text(self, markdown: str) -> bool:
        """First signal of the figure gate: does the figure's own OCR read like a body of text?

        A photo yields nothing or a stray word, so it fails here and never reaches the inner layout
        pass, which keeps the fourth pass free for pages that have no dense figure at all.
        """
        return looks_like_body_text(
            markdown,
            min_chars=self.figure_text_min_chars,
            min_lines=self.figure_text_min_lines,
            min_alnum_ratio=self.figure_text_min_alnum_ratio,
        )

    def _figure_regions(
            self,
            image: Image.Image,
            page_crops: list[tuple[Image.Image, LayoutBox]],
            page_markdowns: list[str],
            ink: Optional[np.ndarray] = None,
    ) -> list[tuple[Image.Image, LayoutBox]]:
        """Fourth pass, second half of the gate: split a text-dense figure into its inner text lines.

        Both signals must agree — the figure's OCR reads as text (above) and the line scan actually
        finds labelled text inside it — otherwise the figure stays one image block, exactly as before.
        Inner boxes are only ever added: the figure block stays, so a wrongly accepted figure costs
        covers, never content.

        The lines come from a morphological scan rather than the layout model: PP-DocLayoutV3 answers
        "this is a figure" and nothing else, no matter how the crop is presented (see textlines.py).

        ``chart`` is left alone: its prompt returns a reading of the chart (a description or its data)
        rather than the labels on it, so there is nothing trustworthy to re-detect inside.
        """
        extra: list[tuple[Image.Image, LayoutBox]] = []
        page_rgb: Optional[np.ndarray] = None
        for (crop, box), markdown in zip(page_crops, page_markdowns):
            if box.label_name != "image":
                continue
            x1, y1, x2, y2 = box.int_rect
            if min(x2 - x1, y2 - y1) < self.figure_min_side:
                continue
            if not self._figure_markdown_is_text(markdown):
                continue
            lines = group_lines(
                detect_text_lines(
                    crop,
                    merge_px=self.figure_line_merge_px,
                    min_height=self.figure_line_min_height,
                    max_height=self.figure_line_max_height,
                    min_width=self.figure_line_min_width,
                    min_ink=self.figure_line_min_ink,
                    max_ink=self.figure_line_max_ink,
                ),
                gap_ratio=self.figure_line_gap_ratio,
                min_x_overlap=self.figure_line_min_x_overlap,
            )
            inner = [
                LayoutBox(
                    xyxy=np.array(
                        [x1 + lx1, y1 + ly1, x1 + lx2, y1 + ly2], dtype=np.float32
                    ),
                    label_id=-1,
                    label_name="text",
                    # No model score behind a scan line; mid-range so it survives the box filter and
                    # never outranks a real detection of the same area.
                    score=0.5,
                    origin=ORIGIN_FIGURE,
                )
                for lx1, ly1, lx2, ly2 in lines
            ]
            kept = self.box_filter.filter(
                inner, page_size=(image.width, image.height), ink=ink,
            )
            coverage = sum(b.area for b in kept) / box.area if box.area > 0 else 0.0
            accepted = len(kept) >= self.figure_inner_min_boxes or coverage >= self.figure_inner_min_coverage
            logger.info(
                "  [figure] box=(%d,%d,%d,%d) chars=%d lines=%d blocks=%d cov=%.2f -> %s",
                x1, y1, x2, y2, len("".join(markdown.split())),
                len([line for line in markdown.splitlines() if line.strip()]),
                len(kept), coverage, "text" if accepted else "image",
            )
            if not accepted:
                continue
            if page_rgb is None:
                page_rgb = np.asarray(image.convert("RGB"))
            for inner_box in kept:
                inner_crop = self._crop(page_rgb, inner_box)
                if inner_crop is not None:
                    extra.append((inner_crop, inner_box))
        return extra

    @staticmethod
    def _text_char_count(items: Sequence[tuple[LayoutBox, str]]) -> int:
        """Characters of text-family markdown — the budget the dedup is allowed to spend."""
        return sum(
            len("".join(md.split())) for box, md in items if box.label_name in TEXT_LABELS
        )

    def _resolve_page_text(
            self,
            items: list[tuple[LayoutBox, str]],
    ) -> PageRegions:
        """Post-OCR cleanup (boxes.py): drop punctuation-only boxes, merge near-duplicate text.

        Geometry alone cannot separate "the same line detected twice with an offset" from two boxes
        that merely touch, and the two recall passes are what produce the former — so the decision is
        made here, where the text can arbitrate.
        """
        return resolve_page_regions(
            items,
            min_overlap=self.dedup_text_min_overlap,
            ratio=self.dedup_text_ratio,
            token_containment=self.dedup_text_token_containment,
            fuzzy_prefix=self.dedup_text_fuzzy_prefix,
            min_content_chars=self.dedup_min_content_chars,
            placeholders=self.dedup_placeholders,
        )

    def process_page(self, image: Image.Image) -> PageResult:
        return self.process_pages([image])[0]

    def process_pages(self, images: Sequence[Image.Image]) -> list[PageResult]:
        """Process a batch end-to-end (cross-page tensor stacking + cross-page label bucketing).

        Layout is one stacked forward for the whole batch and VL crops are bucketed by label across
        pages (each forward capped by max_forward_batch). VRAM grows with batch size; 4 pages is safe
        on an 8GB card (single-page peak ~3.1GB with both backbones resident). Each page's
        ``elapsed_seconds`` is the whole-batch time.
        """
        if not images:
            return []
        st = time.perf_counter()
        images = [im.convert("RGB") for im in images]

        layouts_per_page = self.layout.detect(images)

        # One ink mask per page: the merge trims boxes to it and the figure line scan reads from it
        ink_per_page = [ink_mask(image) for image in images]

        pages_crops = [
            self._crop_page(image, layouts, ink)
            for image, layouts, ink in zip(images, layouts_per_page, ink_per_page)
        ]

        markdowns = self.vl.recognize_grouped(
            [(img, box.label_name) for page_crops in pages_crops for img, box in page_crops]
        )

        # Second VL round, only for the inner text of figures that passed the density gate (usually
        # zero or one page per batch, so it costs one small extra forward)
        cursor = 0
        pages_markdowns: list[list[str]] = []
        for page_crops in pages_crops:
            pages_markdowns.append(markdowns[cursor:cursor + len(page_crops)])
            cursor += len(page_crops)
        figure_items = [
            self._figure_regions(image, page_crops, page_markdowns, ink)
            for image, page_crops, page_markdowns, ink in zip(
                images, pages_crops, pages_markdowns, ink_per_page
            )
        ]
        figure_markdowns = self.vl.recognize_grouped(
            [(img, box.label_name) for page_items in figure_items for img, box in page_items]
        )
        cursor = 0
        for page_items, page_crops, page_markdowns in zip(figure_items, pages_crops, pages_markdowns):
            took = len(page_items)
            page_markdowns.extend(figure_markdowns[cursor:cursor + took])
            page_crops.extend(page_items)
            cursor += took

        elapsed = time.perf_counter() - st
        results: list[PageResult] = []
        for image, page_crops, page_markdowns in zip(images, pages_crops, pages_markdowns):
            items = [(box, md) for (_, box), md in zip(page_crops, page_markdowns)]
            resolved = self._resolve_page_text(items)
            if resolved.merged or resolved.junk:
                before = self._text_char_count(items)
                after = self._text_char_count(resolved.regions)
                loss = (before - after) / before if before else 0.0
                logger.info(
                    "  [dedup] %d -> %d regions (merged %d, junk %d), text chars %d -> %d (%.1f%%)",
                    len(items), len(resolved.regions), len(resolved.merged), len(resolved.junk),
                    before, after, -100.0 * loss,
                )
                if loss > self.dedup_text_max_loss:
                    # The merged text is the reading the survivor already carries; a large share means
                    # the page repeats itself or a box was swallowed that should have stood alone.
                    for box, md in resolved.merged[:5]:
                        x1, y1, _, _ = box.int_rect
                        logger.warning(
                            "  [dedup] merged away %s at (%d,%d): %r",
                            box.label_name, x1, y1, md[:60],
                        )
                    logger.warning(
                        "  [dedup] dropped %.1f%% of the page's text (limit %.1f%%) — check the merge "
                        "thresholds in app/config.py",
                        100.0 * loss, 100.0 * self.dedup_text_max_loss,
                    )
            results.append(
                PageResult(
                    width=image.width,
                    height=image.height,
                    elapsed_seconds=elapsed,
                    regions=[
                        RegionResult(
                            label=box.label_name,
                            score=box.score,
                            rect=box.int_rect,
                            markdown=md,
                        )
                        for box, md in resolved.regions
                    ],
                )
            )
        return results
