"""OCR 引擎单例：懒加载 + 单锁串行。

退化自 Wise-Paddle 的 PipelinePool + BatchScheduler——ezpdf 中 Rust 是唯一
调度者、无多用户并发，一个 pipeline 实例 + threading.Lock 足够。
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
        """首次调用才吃显存（拓扑 A：引擎懒加载）。加载耗时以分钟计。"""
        with self._lock:
            if self._pipe is None:
                import torch  # 懒加载：torch 未装/未导入不阻塞服务启动

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
        with self._lock:  # 单实例推理串行
            return pipe.process_page(image)

    def recognize_batch(self, images: Sequence) -> list[PageResult]:
        """多页批量推理：layout 跨页堆叠 + VL 跨页 label 分桶（同锁串行整批）。"""
        pipe = self.load()
        with self._lock:
            return pipe.process_pages(images)


engine = Engine()
