"""EZPDF pyserver 环境探测脚本。

零第三方依赖（纯 stdlib），venv 内外均可运行：
    python bootstrap.py          # stdout 单行 JSON（Rust 解析）
    python bootstrap.py | python -m json.tool   # 人工查看

模型目录取 EZPDF_MODELS_DIR（生产 = ~/.ezpdf/models），未设时用 <脚本目录>/models。

输出契约（供 Rust 的 ocr_env_report 合成前端状态灯数据）：
    {
      "python": "3.12.10", "python_path": "...",
      "deps": {"fastapi": "0.139.2", ...},     # 版本号，缺失为 null
      "missing": ["torch"],
      "torch_build": "cuda" | "cpu" | null,    # torch 版本 +cu 后缀判定，不 import torch
      "gpu": {"present": true, "name": "...", "driver": "...", "cuda": "13.2"} | null,
      "models": {"layout": true, "vl": false},
      "error": null                            # 探测本身出错时的兜底信息
    }
"""

from __future__ import annotations

import importlib.metadata
import importlib.util
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from app import model_contract

# (发行名, 导入名)；版本取发行名。torch 在列——bootstrap 只看版本元数据，不 import
DEPS: list[tuple[str, str]] = [
    ("fastapi", "fastapi"),
    ("uvicorn", "uvicorn"),
    ("transformers", "transformers"),
    ("pillow", "PIL"),
    ("numpy", "numpy"),
    ("safetensors", "safetensors"),
    ("torch", "torch"),
]

# 模型目录约定：EZPDF_MODELS_DIR 覆盖（生产 = ~/.ezpdf/models），默认 <pyserver>/models；
# 目录名与完整性口径来自 app/model_contract.py（与下载入口 fetch.py 同一份，纯标准库）
MODEL_ROOT = Path(os.environ.get("EZPDF_MODELS_DIR") or (Path(__file__).resolve().parent / "models"))
MODEL_DIRS: dict[str, str] = {
    "layout": model_contract.LAYOUT_MODEL_DIR_NAME,
    "vl": model_contract.VL_MODEL_DIR_NAME,
}

_CREATE_NO_WINDOW = 0x08000000  # 隐藏子进程控制台（与 Rust CREATE_NO_WINDOW 同值）


def _dep_report() -> tuple[dict[str, str | None], list[str]]:
    deps: dict[str, str | None] = {}
    missing: list[str] = []
    for dist, import_name in DEPS:
        if importlib.util.find_spec(import_name) is None:
            deps[dist] = None
            missing.append(dist)
            continue
        try:
            deps[dist] = importlib.metadata.version(dist)
        except importlib.metadata.PackageNotFoundError:
            deps[dist] = None
            missing.append(dist)
    return deps, missing


def _has_bundled_cuda() -> bool:
    """PyPI 的 Linux torch 轮子**捆绑 CUDA** 却不带 `+cu` 本地版本号（实测 2.13.0
    自述 torch.version.cuda=13.2），只能靠随它装进来的 nvidia-* / triton 判断。"""
    try:
        for dist in importlib.metadata.distributions():
            name = (dist.metadata["Name"] or "").lower()
            if name.startswith("nvidia-") or name == "triton":
                return True
    except Exception:
        return False
    return False


def _torch_build(deps: dict[str, str | None]) -> str | None:
    """torch 构建变体：+cuNNN 后缀 = cuda 构建，+cpu = cpu 构建；未装 = None。"""
    version = deps.get("torch")
    if version is None:
        return None
    if "+cu" in version:
        return "cuda"
    if "+cpu" in version:
        return "cpu"
    return "cuda" if _has_bundled_cuda() else "cpu"


def _gpu_report() -> dict | None:
    """nvidia-smi 探测：无卡/无驱动/调用失败一律返回 None（常态，不算错误）。"""
    exe = shutil.which("nvidia-smi")
    if exe is None:
        return None
    kwargs = {"capture_output": True, "text": True, "timeout": 10}
    if os.name == "nt":
        kwargs["creationflags"] = _CREATE_NO_WINDOW
    try:
        query = subprocess.run(
            [exe, "--query-gpu=name,driver_version", "--format=csv,noheader"],
            **kwargs,
        )
        if query.returncode != 0:
            return None
        name, driver = (query.stdout.splitlines()[0].split(","))[:2]
        # 驱动可跑的 CUDA 版本在 nvidia-smi 表头（首行是日期，CUDA Version 在其下几行内）
        bare = subprocess.run([exe], **kwargs)
        cuda = None
        for line in (bare.stdout.splitlines() or [])[:5]:
            if "CUDA Version:" in line:
                cuda = line.split("CUDA Version:")[1].strip().split()[0]
                break
        return {"present": True, "name": name.strip(), "driver": driver.strip(), "cuda": cuda}
    except Exception:
        return None


def probe() -> dict:
    deps, missing = _dep_report()
    return {
        "python": ".".join(map(str, sys.version_info[:3])),
        "python_path": sys.executable,
        "deps": deps,
        "missing": missing,
        "torch_build": _torch_build(deps),
        "gpu": _gpu_report(),
        "models": {key: model_contract.model_dir_ok(MODEL_ROOT / name) for key, name in MODEL_DIRS.items()},
        "error": None,
    }


def main() -> int:
    try:
        report = probe()
    except Exception as exc:  # 探测自身崩溃也必须吐 JSON，且以成功码退出（报告即数据）
        # 契约仍是完整报告：缺字段会让 Rust 侧反序列化失败，真正的原因就到不了界面
        report = {
            "python": None,
            "python_path": None,
            "deps": {},
            "missing": [],
            "torch_build": None,
            "gpu": None,
            "models": None,
            "error": f"{type(exc).__name__}: {exc}",
        }
    print(json.dumps(report, ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
