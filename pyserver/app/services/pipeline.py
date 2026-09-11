"""OCR 流水线（移植自 Wise-Paddle core_pipeline.py，删除并发与落盘设施）。

Pipeline tree (per page):

    PIL.Image (任意尺寸)
        │
        ▼
    LayoutDetector  ── 内部 processor 自动 resize 到 800x800
        │ boxes (float xyxy)  / labels / scores
        ▼
    ┌─ 转 int rectangle (np.round → int) ─┐
    │  NMS (IoU)                          │  BoxFilter
    │  面积阈值                            │
    │  分数阈值                            │
    └────────────────────────────────────┘
        │ 保留的 LayoutBox
        ▼
    RegionCropper  ── numpy 切片裁剪 (RGB/HWC uint8)
        │
        ▼
    按 label 分桶 → 同桶 batch 提交给 VLPredictor (PaddleOCR-VL-1.6)
        │
        ▼
    每张裁剪图得到一段 markdown → 内存 RegionResult（不落盘）

相对原版的退化：删除 PipelinePool / BatchScheduler / Job / voucher /
text_result 落盘——ezpdf 中 Rust 是唯一调度者，结果经 HTTP 直接返回。
精度关键设施（BoxFilter 的 NMS + unclip 扩框）原样保留。
"""

from __future__ import annotations

import logging
import time
from dataclasses import dataclass, field
from typing import Optional, Sequence

import numpy as np
import torch
from PIL import Image
from transformers import (
    AutoImageProcessor,
    AutoModelForObjectDetection,
    AutoModelForImageTextToText,
    AutoProcessor,
)

logger = logging.getLogger("ezpdf.pipeline")


# ─────────────────────────────────────────────────────────────────────────────
# 兼容性补丁：transformers 5.x 不再带 "default" rope init 入口，老模型需要补
# ─────────────────────────────────────────────────────────────────────────────
def _patch_rope_default() -> None:
    """Patch transformers 5.x to restore the missing 'default' RoPE init entry.

    Older model configs reference ``rope_type="default"`` which was removed in
    transformers 5.x. We re-register an equivalent implementation so those
    configs keep loading without manual edits.
    """
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


# Module-level patch; idempotent and safe to call multiple times.
_patch_rope_default()


# ─────────────────────────────────────────────────────────────────────────────
# 数据类
# ─────────────────────────────────────────────────────────────────────────────
@dataclass
class LayoutBox:
    """单个版面区域。"""

    xyxy: np.ndarray  # float32, 形状 (4,) —— [x1, y1, x2, y2]（原图坐标系）
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
    """一个裁剪区域的最终结果。"""

    page_index: int
    box_index: int
    label: str
    score: float
    rect: tuple[int, int, int, int]
    crop_shape: tuple[int, int]
    markdown: str


@dataclass
class PageResult:
    """一页的处理结果。"""

    page_index: int
    width: int
    height: int
    elapsed_seconds: float
    regions: list[RegionResult] = field(default_factory=list)

    @property
    def markdown(self) -> str:
        return "\n\n".join(r.markdown for r in self.regions)


# ─────────────────────────────────────────────────────────────────────────────
# 1) Layout detection —— PP-DocLayoutV3
# ─────────────────────────────────────────────────────────────────────────────
class LayoutDetector:
    """把任意尺寸的图过 PP-DocLayoutV3，返回 (box, label, score) 三元组。"""

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
        # processor 内置 800x800 resize；只要原图长边合理，processor 自动适配
        self._max_long_side = 1600  # 防止 4K+ 大图把显存打爆

    def _maybe_downscale(self, image: Image.Image) -> Image.Image:
        """If the image's long side exceeds ``_max_long_side``, downscale it.

        Returns the original image unchanged when it is already small enough.
        """
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
    def detect(self, images: Sequence[Image.Image]) -> list[list[LayoutBox]]:
        """Run layout detection on a batch of images.

        Args:
            images: One or more PIL images (any size, any mode).

        Returns:
            A list (one entry per input image) of ``LayoutBox`` lists.
        """
        if not images:
            return []
        scaled = [self._maybe_downscale(im.convert("RGB")) for im in images]

        inputs = self.processor(images=scaled, return_tensors="pt").to(self.device)
        target_sizes = torch.tensor(
            [[im.height, im.width] for im in images], device=self.device
        )
        outputs = self.model(**inputs)
        # post_process_object_detection 在 threshold 处做一次初筛；后面 BoxFilter
        # 再做更细的 IoU / 面积过滤
        raw = self.processor.post_process_object_detection(
            outputs, target_sizes=target_sizes, threshold=self.score_threshold
        )

        out: list[list[LayoutBox]] = []
        for r, src in zip(raw, images):
            boxes = r["boxes"].detach().cpu().numpy()
            labels = r["labels"].detach().cpu().numpy()
            scores = r["scores"].detach().cpu().numpy()
            page_boxes: list[LayoutBox] = []
            for box, lid, sc in zip(boxes, labels, scores):
                x1, y1, x2, y2 = box
                # 截到原图边界内
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


# ─────────────────────────────────────────────────────────────────────────────
# 2) Box filter —— NMS + 面积 / 分数门槛
# ─────────────────────────────────────────────────────────────────────────────
class BoxFilter:
    """纯 numpy NMS + 过滤；不依赖额外库。

    可调精度参数（影响 layout 召回/裁剪质量）：

    - ``iou_threshold``: NMS 重叠上限（越大越激进去重）
    - ``min_area``:      最小框面积（像素²）
    - ``min_score``:     最低置信度
    - ``unclip_ratio``:  NMS 后把框向外扩的比例（0.05 = 每边扩 5%）。给 VL 更多
                        上下文，提升 OCR 准确率；过大会把别的 region 也包进来。
                        doclayout 边界偏紧时这个最有用。
    - ``expand_pixels``: 每边再多扩 N 个像素（绝对值）。和 ratio 叠加生效。
    """

    def __init__(
            self,
            iou_threshold: float = 0.5,
            min_area: float = 16 * 16,
            min_score: float = 0.5,
            unclip_ratio: float = 0.0,
            expand_pixels: float = 0.0,
    ) -> None:
        self.iou_threshold = float(iou_threshold)
        self.min_area = float(min_area)
        self.min_score = float(min_score)
        self.unclip_ratio = float(unclip_ratio)
        self.expand_pixels = float(expand_pixels)

    @staticmethod
    def _iou(a: np.ndarray, b: np.ndarray) -> float:
        """Compute IoU between two xyxy boxes (float arrays of length 4)."""
        ax1, ay1, ax2, ay2 = a
        bx1, by1, bx2, by2 = b
        ix1, iy1 = max(ax1, bx1), max(ay1, by1)
        ix2, iy2 = min(ax2, bx2), min(ay2, by2)
        iw, ih = max(0.0, ix2 - ix1), max(0.0, iy2 - iy1)
        inter = iw * ih
        union = (ax2 - ax1) * (ay2 - ay1) + (bx2 - bx1) * (by2 - by1) - inter
        return inter / union if union > 0 else 0.0

    def _unclip(self, box: np.ndarray) -> np.ndarray:
        """按 ratio + 绝对像素把框向外扩，返回 (x1,y1,x2,y2)。"""
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
        """过滤 + NMS + 可选 unclip。

        Args:
            boxes: 待过滤的 ``LayoutBox`` 列表。
            page_size: ``(W, H)`` 可选；给 unclip 提供边界 clamp（防止扩出图外）。

        Returns:
            过滤后保留的 ``LayoutBox`` 列表（按分数降序）。
        """
        # 先按分数从高到低
        keep: list[LayoutBox] = []
        candidates = sorted(boxes, key=lambda b: b.score, reverse=True)
        for b in candidates:
            if b.score < self.min_score or b.area < self.min_area:
                continue
            if any(self._iou(b.xyxy, k.xyxy) > self.iou_threshold for k in keep):
                continue
            # unclip：在 NMS 之后，避免影响 NMS 决策
            if self.unclip_ratio > 0 or self.expand_pixels > 0:
                expanded = self._unclip(b.xyxy).copy()
                if page_size is not None:
                    w, h = page_size
                    expanded[0] = max(0.0, expanded[0])
                    expanded[1] = max(0.0, expanded[1])
                    expanded[2] = min(float(w), expanded[2])
                    expanded[3] = min(float(h), expanded[3])
                b = LayoutBox(
                    xyxy=expanded,
                    label_id=b.label_id,
                    label_name=b.label_name,
                    score=b.score,
                )
            keep.append(b)
        return keep


# ─────────────────────────────────────────────────────────────────────────────
# 3) Region cropper —— numpy 切片
# ─────────────────────────────────────────────────────────────────────────────
class RegionCropper:
    """用 numpy 把 LayoutBox 对应的区域切出来，返回 RGB ``np.ndarray``。"""

    def crop(self, rgb: np.ndarray, box: LayoutBox) -> np.ndarray:
        """Crop ``box`` from an RGB ``(H, W, 3)`` uint8 array.

        Returns a 1×1 black placeholder if the clamped box collapses to empty.
        """
        x1, y1, x2, y2 = box.int_rect
        # clamp 到合法范围
        h, w = rgb.shape[:2]
        x1 = max(0, min(x1, w))
        x2 = max(0, min(x2, w))
        y1 = max(0, min(y1, h))
        y2 = max(0, min(y2, h))
        if x2 <= x1 or y2 <= y1:
            return np.zeros((1, 1, 3), dtype=np.uint8)
        return rgb[y1:y2, x1:x2].copy()


# ─────────────────────────────────────────────────────────────────────────────
# 4) VL predictor —— PaddleOCR-VL-1.6，按 label 分桶后 batch
# ─────────────────────────────────────────────────────────────────────────────
class VLPredictor:
    """包裹 PaddleOCR-VL-1.6，给一批裁剪图做 VL 识别。"""

    # PaddleOCR-VL 官方推荐的 task prompt（来自 PaddleOCR-VL-1.6 README）
    DEFAULT_PROMPTS: dict[str, str] = {
        "text": "OCR:",
        "paragraph_title": "OCR:",
        "doc_title": "OCR:",
        "table": "Table Recognition:",
        "formula": "Formula Recognition:",
        "image": "OCR:",
        "chart": "Chart Recognition:",
        "abstract": "OCR:",
        "reference": "OCR:",
        "reference_content": "OCR:",
        "footer": "OCR:",
        "header": "OCR:",
        "footnote": "OCR:",
        "seal": "OCR:",
        "number": "OCR:",
        "_default": "OCR:",
    }

    def __init__(
            self,
            model_path: str,
            device: torch.device,
            prompts: Optional[dict[str, str]] = None,
            dtype: torch.dtype = torch.bfloat16,
            max_new_tokens: int = 256,
            max_pixels: int = 1280 * 28 * 28,  # 官方默认 1MP（longest_edge）
            min_pixels: int = 112896,  # 官方默认 shortest_edge
            max_forward_batch: int = 10,  # 单次 VL forward 最多 image 数（显存上限）
            attn_impl: str = "sdpa",
            repetition_penalty: float = 1.15,  # 防止 batch 推理陷入重复循环
            do_sample: bool = False,  # greedy 解码；想更"活"可以改 True
    ) -> None:
        logger.info("Loading VL model: %s", model_path)
        self.processor = AutoProcessor.from_pretrained(model_path)
        # 关键：decoder-only 必须 left-padding，否则 batch 推理全乱
        try:
            self.processor.tokenizer.padding_side = "left"
        except Exception:
            pass
        # 关键：必须用 AutoModelForImageTextToText，不是 AutoModelForCausalLM
        # 之前的 CausalLM 入口导致 model 看不到 image，全靠编造
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
        # EOS：模型的 generation_config.json 里写的是 </s> (id=2)
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
        """给每张图造一个 messages（apply_chat_template batched 模式需要 list of conversations）。"""
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
        """单次 VL forward，调用方负责保证 ``len(images) <= self.max_forward_batch``。"""
        if not images:
            return []
        conversations = self._build_messages(images, label)
        inputs = self.processor.apply_chat_template(
            conversations,
            add_generation_prompt=True,
            tokenize=True,
            return_dict=True,
            return_tensors="pt",
            # transformers 5.x: 传给 processor.__call__ 的处理参数必须放进
            # ``processor_kwargs`` dict，否则会收到弃用告警（Kwargs passed to
            # `processor.__call__` have to be in `processor_kwargs`）
            processor_kwargs={
                "padding": True,  # batch 推理要 padding
                "images_kwargs": {
                    "size": {
                        "shortest_edge": self.min_pixels,
                        "longest_edge": self.max_pixels,
                    }
                },
            },
        ).to(self.device)
        ids = inputs["input_ids"]
        # image token 实际通过 mm_token_type_ids 标记 (apply_chat_template 不插入 <|image_pad|>)
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
        """同 label 的图用同一 prompt 做 batch 推理。len > max_forward_batch 时拆 sub-batch。

        例：12 张 text crop, max_forward_batch=4 → 3 次 VL forward (4+4+4)
        """
        if not images:
            return []
        cap = self.max_forward_batch
        if len(images) <= cap:
            return self._forward_once(images, label)
        # 拆 sub-batch
        out: list[str] = []
        for start in range(0, len(images), cap):
            chunk = list(images[start:start + cap])
            out.extend(self._forward_once(chunk, label))
        return out

    @torch.no_grad()
    def recognize_grouped(
            self, items: list[tuple[Image.Image, str]]
    ) -> list[str]:
        """按 label 分桶 → 每个桶一次 batch 推理 → 按原顺序还原。"""
        # 桶：(label -> [(idx, img)])
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


# ─────────────────────────────────────────────────────────────────────────────
# 5) Pipeline —— 把上述 4 步串成一条主干
# ─────────────────────────────────────────────────────────────────────────────
class OCRPipeline:
    """对一张图跑 layout detection → 过滤 → 裁剪 → VL 识别 → 内存结果。

    所有可调精度/速度/显存参数都从构造参数传进来（默认值对齐 Wise-Paddle）。
    """

    def __init__(
            self,
            layout_model_path: str,
            vl_model_path: str,
            device: torch.device,
            # ---- LayoutDetector ----
            score_threshold: float = 0.5,
            # ---- BoxFilter ----
            box_iou_threshold: float = 0.5,
            box_min_area: float = 16 * 16,
            box_min_score: float = 0.5,
            box_unclip_ratio: float = 0.05,  # 每边向外扩 5%，提精度
            box_expand_pixels: float = 4.0,  # 额外每边扩 N 像素
            # ---- VLPredictor ----
            max_new_tokens: int = 256,
            vl_min_pixels: int = 112896,
            vl_max_pixels: int = 1280 * 28 * 28,
            vl_max_forward_batch: int = 4,
            vl_repetition_penalty: float = 1.15,
            vl_do_sample: bool = False,
            # ---- runtime ----
            max_regions: int = 100,
            dtype: torch.dtype = torch.bfloat16,
            attn_impl: str = "sdpa",
    ) -> None:
        self.device = device
        self.layout = LayoutDetector(
            layout_model_path, device, score_threshold=score_threshold,
        )
        # NOTE: 不用 self.filter —— 会 shadow Python 内置 filter()
        self.box_filter = BoxFilter(
            iou_threshold=box_iou_threshold,
            min_area=box_min_area,
            min_score=box_min_score,
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
        """Filter boxes on one page and crop them into ``(PIL.Image, LayoutBox)`` pairs."""
        kept = self.box_filter.filter(
            layouts, page_size=(image.width, image.height)
        )[: self.max_regions]
        crops: list[tuple[Image.Image, LayoutBox]] = []
        # 每页只做一次 RGB 转换
        rgb = np.asarray(image.convert("RGB"))
        for box in kept:
            arr = self.cropper.crop(rgb, box)
            if arr.size == 0 or arr.shape[0] < 2 or arr.shape[1] < 2:
                continue
            crops.append((Image.fromarray(arr), box))
        return crops

    def process_page(
            self,
            image: Image.Image,
            page_index: int = 0,
    ) -> PageResult:
        """Process a single image end-to-end. 结果全内存返回，不落盘。"""
        result = self.process_pages([image], page_index_base=page_index)
        return result[0]

    def process_pages(
            self,
            images: Sequence[Image.Image],
            page_index_base: int = 0,
    ) -> list[PageResult]:
        """Process a batch of images end-to-end（跨页张量堆叠 + 跨页 label 分桶）。

        相对逐页调 ``process_page`` 的收益：

        - layout 检测：整批一次 stacked forward（processor 内部统一 resize 到
          800x800，批内图一次吃进显存）；
        - VL 识别：跨页按 label 分桶（``recognize_grouped``），同桶 crop 跨页
          合并后受 ``max_forward_batch`` 控制单次 forward 规模；
        - 显存代价随批大小增长（layout 批 + VL 桶 batch），批 4 页在 8GB 卡上
          实测安全（单页引擎峰值 ~3.1GB，layout/VL 骨干为常驻部分）。

        各页 ``elapsed_seconds`` 均为整批耗时（layout 是联合 forward，不可按页拆分）。
        """
        st = time.perf_counter()
        images = [im.convert("RGB") for im in images]
        if not images:
            return []

        # 1) layout detection：整批一次 stacked forward（内部自动 resize）
        layouts_per_page = self.layout.detect(images)

        # 2) 每页独立 filter + crop
        pages_crops = [
            self._crop_page(image, layouts)
            for image, layouts in zip(images, layouts_per_page)
        ]

        # 3) 跨页 label 分桶 → VL batch 推理
        all_items = [
            (img, box.label_name)
            for page_crops in pages_crops
            for img, box in page_crops
        ]
        markdowns = self.vl.recognize_grouped(all_items)

        # 4) 按页还原 RegionResult（markdowns 与 all_items 同序，游标切片还原）
        elapsed = time.perf_counter() - st
        results: list[PageResult] = []
        cursor = 0
        for offset, (image, page_crops) in enumerate(zip(images, pages_crops)):
            count = len(page_crops)
            regions = [
                RegionResult(
                    page_index=page_index_base + offset,
                    box_index=c_idx,
                    label=box.label_name,
                    score=box.score,
                    rect=box.int_rect,
                    crop_shape=(img.height, img.width),
                    markdown=md,
                )
                for c_idx, ((img, box), md) in enumerate(
                    zip(page_crops, markdowns[cursor:cursor + count])
                )
            ]
            cursor += count
            results.append(
                PageResult(
                    page_index=page_index_base + offset,
                    width=image.width,
                    height=image.height,
                    elapsed_seconds=elapsed,
                    regions=regions,
                )
            )
        return results
