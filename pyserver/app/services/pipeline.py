"""OCR pipeline (ported from Wise-Paddle core_pipeline.py; concurrency and disk output removed).

Per page: PP-DocLayoutV3 layout → BoxFilter (NMS + thresholds) → numpy crop → PaddleOCR-VL-1.6
batched per label → in-memory RegionResult (never written to disk). Rust is the sole scheduler and
results return over HTTP; the accuracy-critical BoxFilter NMS + unclip are kept as-is.
"""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
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


@dataclass
class LayoutBox:
    xyxy: np.ndarray  # float32, shape (4,) — [x1, y1, x2, y2] in source-image coordinates
    label_id: int
    label_name: str
    score: float

    @property
    def int_rect(self) -> tuple[int, int, int, int]:
        return tuple(int(round(float(v))) for v in self.xyxy)

    @property
    def area(self) -> float:
        x1, y1, x2, y2 = self.xyxy
        return max(0.0, x2 - x1) * max(0.0, y2 - y1)


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
    regions: list[RegionResult] = field(default_factory=list)


def _iou_xyxy(a: np.ndarray, b: np.ndarray) -> float:
    ax1, ay1, ax2, ay2 = a
    bx1, by1, bx2, by2 = b
    ix1, iy1 = max(ax1, bx1), max(ay1, by1)
    ix2, iy2 = min(ax2, bx2), min(ay2, by2)
    iw, ih = max(0.0, ix2 - ix1), max(0.0, iy2 - iy1)
    inter = iw * ih
    union = (ax2 - ax1) * (ay2 - ay1) + (bx2 - bx1) * (by2 - by1) - inter
    return inter / union if union > 0 else 0.0


def _containment(a: np.ndarray, b: np.ndarray) -> float:
    """Fraction of a covered by b (inter / area(a)); drops duplicate "line inside a block" candidates."""
    ax1, ay1, ax2, ay2 = a
    bx1, by1, bx2, by2 = b
    iw = max(0.0, min(ax2, bx2) - max(ax1, bx1))
    ih = max(0.0, min(ay2, by2) - max(ay1, by1))
    area = max(0.0, ax2 - ax1) * max(0.0, ay2 - ay1)
    return (iw * ih) / area if area > 0 else 0.0


class LayoutDetector:
    def __init__(
            self,
            model_path: str,
            device: torch.device,
            score_threshold: float = 0.5,
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
        self._max_long_side = 1600  # cap so 4K+ images cannot blow up VRAM
        # Second-pass recall params (see detect); tune sensitivity here
        self._fallback_labels = frozenset({
            "text", "paragraph_title", "doc_title", "abstract", "aside_text",
            "footnote", "figure_title", "content", "reference", "reference_content",
        })
        self._fallback_threshold = 0.45
        self._fallback_sharpen = (2, 300, 1)  # PIL UnsharpMask: radius, percent, threshold
        self._fallback_iou = 0.3
        self._fallback_containment = 0.6
        # Third pass, half-page tiles (see detect); add-only, and its whitelist also takes header/footer
        # because a cover's standard line is classified as header
        self._tile_labels = self._fallback_labels | {"header", "footer"}
        self._tile_threshold = 0.38
        self._tile_overlap = 0.08
        self._tile_iou = 0.3
        self._tile_containment = 0.6
        # batch=8 hits a slow kernel (measured 6.0s vs 0.22s at batch=4), so all forwards are chunked
        self._max_forward_batch = 4

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
    ) -> list[list[LayoutBox]]:
        """Chunked forward (≤ ``_max_forward_batch``); the slow batch=8 kernel is why, see _max_forward_batch."""
        images = list(images)
        if len(images) <= self._max_forward_batch:
            return self._forward_one(images, target_sizes, threshold)
        out: list[list[LayoutBox]] = []
        for i in range(0, len(images), self._max_forward_batch):
            chunk = images[i:i + self._max_forward_batch]
            out.extend(
                self._forward_one(chunk, target_sizes[i:i + len(chunk)], threshold)
            )
        return out

    @torch.no_grad()
    def _forward_one(
            self,
            images: Sequence[Image.Image],
            target_sizes: torch.Tensor,
            threshold: float,
    ) -> list[list[LayoutBox]]:
        inputs = self.processor(images=list(images), return_tensors="pt").to(self.device)
        outputs = self.model(**inputs)
        # post_process does a first coarse cut at threshold; BoxFilter filters IoU/area finer afterwards
        raw = self.processor.post_process_object_detection(
            outputs, target_sizes=target_sizes, threshold=threshold
        )

        out: list[list[LayoutBox]] = []
        for r, src in zip(raw, images):
            boxes = r["boxes"].detach().cpu().numpy()
            labels = r["labels"].detach().cpu().numpy()
            scores = r["scores"].detach().cpu().numpy()
            page_boxes: list[LayoutBox] = []
            for box, lid, sc in zip(boxes, labels, scores):
                x1, y1, x2, y2 = box
                x1 = max(0.0, min(float(x1), src.width))
                x2 = max(0.0, min(float(x2), src.width))
                y1 = max(0.0, min(float(y1), src.height))
                y2 = max(0.0, min(float(y2), src.height))
                page_boxes.append(
                    LayoutBox(
                        xyxy=np.array([x1, y1, x2, y2], dtype=np.float32),
                        label_id=int(lid),
                        label_name=self.id2label.get(int(lid), str(int(lid))),
                        score=float(sc),
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
    ) -> None:
        """Add-only merge: whitelisted label and low overlap with existing/added boxes (both IoU and containment)."""
        if candidate.label_name not in labels:
            return
        for k in page_boxes:
            if _iou_xyxy(candidate.xyxy, k.xyxy) > iou_limit:
                return
            if _containment(candidate.xyxy, k.xyxy) > containment_limit:
                return
        page_boxes.append(candidate)

    def _split_tiles(self, image: Image.Image) -> tuple[list[Image.Image], list[int]]:
        """Top/bottom half-page tiles (with overlap); returns (tiles, each tile's y offset in the page)."""
        w, h = image.size
        if h < 200:  # too short to tile
            return [], []
        cut = h // 2
        ov = int(h * self._tile_overlap)
        first = image.crop((0, 0, w, min(h, cut + ov)))
        second = image.crop((0, max(0, cut - ov), w, h))
        return [first, second], [0, max(0, cut - ov)]

    @torch.no_grad()
    def detect(self, images: Sequence[Image.Image]) -> list[list[LayoutBox]]:
        """Run layout detection on a batch of images (multi-pass, add-only).

        pdfjs-rendered text scores ~0.1-0.15 below pdfium, so whole blocks go missing; three add-only
        passes (results are never replaced, since sharpening lowers some edge scores): source at 0.5,
        sharpened full image at 0.45 (text-family), sharpened half-page tiles at 0.38.
        Candidates only pass with low overlap vs kept boxes (IoU/containment); whitelists in __init__.
        """
        if not images:
            return []
        scaled = [self._maybe_downscale(im.convert("RGB")) for im in images]

        target_sizes = torch.tensor(
            [[im.height, im.width] for im in images], device=self.device
        )
        out = self._forward(scaled, target_sizes, self.score_threshold)

        fallback = [
            im.filter(ImageFilter.UnsharpMask(*self._fallback_sharpen)) for im in scaled
        ]
        for page_boxes, candidates in zip(
            out, self._forward(fallback, target_sizes, self._fallback_threshold)
        ):
            for b in candidates:
                self._append_candidate(
                    page_boxes, b, self._fallback_labels,
                    self._fallback_iou, self._fallback_containment,
                )

        tiles: list[Image.Image] = []
        offsets_per_page: list[list[int]] = []
        for im in scaled:
            parts, offsets = self._split_tiles(im)
            tiles.extend(parts)
            offsets_per_page.append(offsets)
        if tiles:
            tile_sizes = torch.tensor(
                [[t.height, t.width] for t in tiles], device=self.device
            )
            sharp_tiles = [
                t.filter(ImageFilter.UnsharpMask(*self._fallback_sharpen)) for t in tiles
            ]
            raw_tiles = self._forward(sharp_tiles, tile_sizes, self._tile_threshold)
            cursor = 0
            for page_boxes, offsets in zip(out, offsets_per_page):
                for off in offsets:
                    shift = np.array([0, off, 0, off], dtype=np.float32)
                    for b in raw_tiles[cursor]:
                        self._append_candidate(
                            page_boxes,
                            LayoutBox(
                                xyxy=b.xyxy + shift,
                                label_id=b.label_id,
                                label_name=b.label_name,
                                score=b.score,
                            ),
                            self._tile_labels, self._tile_iou, self._tile_containment,
                        )
                    cursor += 1
        return out


class BoxFilter:
    """Pure numpy NMS + filtering; no extra dependencies.

    min_score sits below the detector's pass thresholds on purpose, leaving a path for the 0.38-0.5
    recall candidates; contain_threshold catches nested boxes NMS can't see; unclip/expand add VL
    context after NMS (too much swallows neighbouring regions).
    """

    def __init__(
            self,
            iou_threshold: float = 0.5,
            min_area: float = 16 * 16,
            min_score: float = 0.35,
            contain_threshold: float = 0.6,
            unclip_ratio: float = 0.0,
            expand_pixels: float = 0.0,
    ) -> None:
        self.iou_threshold = float(iou_threshold)
        self.min_area = float(min_area)
        self.min_score = float(min_score)
        self.contain_threshold = float(contain_threshold)
        self.unclip_ratio = float(unclip_ratio)
        self.expand_pixels = float(expand_pixels)

    def _unclip(self, box: np.ndarray) -> np.ndarray:
        x1, y1, x2, y2 = box
        w = x2 - x1
        h = y2 - y1
        dx = w * self.unclip_ratio + self.expand_pixels
        dy = h * self.unclip_ratio + self.expand_pixels
        return np.array([x1 - dx, y1 - dy, x2 + dx, y2 + dy], dtype=np.float32)

    def filter(
            self,
            boxes: list[LayoutBox],
            page_size: Optional[tuple[int, int]] = None,
    ) -> list[LayoutBox]:
        """Filter + NMS + optional unclip (``page_size`` (W,H) clamps unclip); returns kept boxes in score order."""
        keep: list[LayoutBox] = []
        candidates = sorted(boxes, key=lambda b: b.score, reverse=True)
        for b in candidates:
            if b.score < self.min_score or b.area < self.min_area:
                continue
            if any(_iou_xyxy(b.xyxy, k.xyxy) > self.iou_threshold for k in keep):
                continue
            keep.append(b)

        # Nested dedup, judged by descending area: NMS only sees IoU, which is low for a large box
        # wrapping a small one. Survivors then filter the score-ordered output.
        survivors: list[LayoutBox] = []
        for b in sorted(keep, key=lambda b: b.area, reverse=True):
            if any(
                    b.area <= k.area and _containment(b.xyxy, k.xyxy) > self.contain_threshold
                    for k in survivors
            ):
                continue
            survivors.append(b)
        survivors_ids = {id(b) for b in survivors}
        keep = [b for b in keep if id(b) in survivors_ids]

        # unclip after NMS so it cannot affect NMS decisions
        if self.unclip_ratio > 0 or self.expand_pixels > 0:
            expanded_keep: list[LayoutBox] = []
            for b in keep:
                expanded = self._unclip(b.xyxy).copy()
                if page_size is not None:
                    w, h = page_size
                    expanded[0] = max(0.0, expanded[0])
                    expanded[1] = max(0.0, expanded[1])
                    expanded[2] = min(float(w), expanded[2])
                    expanded[3] = min(float(h), expanded[3])
                expanded_keep.append(LayoutBox(
                    xyxy=expanded,
                    label_id=b.label_id,
                    label_name=b.label_name,
                    score=b.score,
                ))
            keep = expanded_keep
        return keep


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
    """Run layout detection → filter → crop → VL recognition → in-memory results; knobs are constructor args."""

    def __init__(
            self,
            layout_model_path: str,
            vl_model_path: str,
            device: torch.device,
            score_threshold: float = 0.5,
            box_iou_threshold: float = 0.5,
            box_min_area: float = 16 * 16,
            box_min_score: float = 0.35,
            box_contain_threshold: float = 0.6,
            # Fixed small expansion only: proportional expansion grows with the box, so a large text box
            # reaches into small headings inside it and the two white covers overlap
            box_unclip_ratio: float = 0.0,
            box_expand_pixels: float = 2.0,
            # 256 truncates long paragraphs (longest text block measured at 1157 chars); short blocks
            # hit EOS early, so 512 only costs extra decode on long ones
            max_new_tokens: int = 512,
            vl_min_pixels: int = 112896,
            vl_max_pixels: int = 1280 * 28 * 28,
            vl_max_forward_batch: int = 4,
            vl_repetition_penalty: float = 1.15,
            vl_do_sample: bool = False,
            max_regions: int = 100,
            dtype: torch.dtype = torch.bfloat16,
            attn_impl: str = "sdpa",
    ) -> None:
        self.device = device
        self.layout = LayoutDetector(
            layout_model_path, device, score_threshold=score_threshold,
        )
        # NOTE: not self.filter — would shadow the builtin filter()
        self.box_filter = BoxFilter(
            iou_threshold=box_iou_threshold,
            min_area=box_min_area,
            min_score=box_min_score,
            contain_threshold=box_contain_threshold,
            unclip_ratio=box_unclip_ratio,
            expand_pixels=box_expand_pixels,
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

    def _crop_page(
            self,
            image: Image.Image,
            layouts: list[LayoutBox],
    ) -> list[tuple[Image.Image, LayoutBox]]:
        kept = self.box_filter.filter(
            layouts, page_size=(image.width, image.height)
        )[: self.max_regions]
        crops: list[tuple[Image.Image, LayoutBox]] = []
        # one RGB conversion per page
        rgb = np.asarray(image.convert("RGB"))
        for box in kept:
            arr = self.cropper.crop(rgb, box)
            if arr.size == 0 or arr.shape[0] < 2 or arr.shape[1] < 2:
                continue
            crops.append((Image.fromarray(arr), box))
        return crops

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

        pages_crops = [
            self._crop_page(image, layouts)
            for image, layouts in zip(images, layouts_per_page)
        ]

        all_items = [
            (img, box.label_name)
            for page_crops in pages_crops
            for img, box in page_crops
        ]
        markdowns = self.vl.recognize_grouped(all_items)

        elapsed = time.perf_counter() - st
        results: list[PageResult] = []
        cursor = 0
        for image, page_crops in zip(images, pages_crops):
            count = len(page_crops)
            regions = [
                RegionResult(
                    label=box.label_name,
                    score=box.score,
                    rect=box.int_rect,
                    markdown=md,
                )
                for (_img, box), md in zip(page_crops, markdowns[cursor:cursor + count])
            ]
            cursor += count
            results.append(
                PageResult(
                    width=image.width,
                    height=image.height,
                    elapsed_seconds=elapsed,
                    regions=regions,
                )
            )
        return results
