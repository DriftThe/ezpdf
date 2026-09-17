"""OCR engine singleton: lazy load + single-lock serialization; Wise-Paddle's PipelinePool and
BatchScheduler were removed because Rust is the sole scheduler with no multi-user concurrency.
"""

from __future__ import annotations

import threading
from collections.abc import Sequence

from ..config import LAYOUT_MODEL_DIR_NAME, MODELS_DIR, VL_MODEL_DIR_NAME
from .pipeline import OCRPipeline, PageResult


class Engine:
    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._pipe: OCRPipeline | None = None

    def load(self) -> OCRPipeline:
        """VRAM is only claimed on first call (lazy load). Loading takes minutes."""
        with self._lock:
            if self._pipe is None:
                import torch  # lazy: a missing/absent torch does not block service startup

                device = torch.device("cuda" if torch.cuda.is_available() else "cpu")
                self._pipe = OCRPipeline(
                    layout_model_path=str(MODELS_DIR / LAYOUT_MODEL_DIR_NAME),
                    vl_model_path=str(MODELS_DIR / VL_MODEL_DIR_NAME),
                    device=device,
                )
                return self._pipe
            return self._pipe

    def recognize(self, image) -> PageResult:
        pipe = self.load()
        with self._lock:
            return pipe.process_page(image)

    def recognize_batch(self, images: Sequence) -> list[PageResult]:
        """Multi-page batch: layout stacked across pages + VL bucketed by label across pages (whole batch under one lock)."""
        pipe = self.load()
        with self._lock:
            return pipe.process_pages(images)


engine = Engine()
