"""EZPDF pyserver environment probe (stdlib only; runs inside or outside a venv).

    python bootstrap.py        # one-line JSON on stdout, parsed by Rust's ocr_env_report
Models come from EZPDF_MODELS_DIR (production ~/.ezpdf/models), default <script dir>/models.

Output contract:
    {"python", "python_path", "deps", "missing", "torch_build" ("cuda"|"cpu"|null, from the +cu
     suffix — torch is never imported), "gpu", "models" ("layout"/"vl" bools), "error"}
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

# (distribution name, import name); versions come from the distribution metadata, torch is never imported
DEPS: list[tuple[str, str]] = [
    ("fastapi", "fastapi"),
    ("uvicorn", "uvicorn"),
    ("transformers", "transformers"),
    ("pillow", "PIL"),
    ("numpy", "numpy"),
    ("safetensors", "safetensors"),
    ("torch", "torch"),
]

# Dir names and completeness come from app/model_contract.py (shared with fetch.py)
MODEL_ROOT = Path(os.environ.get("EZPDF_MODELS_DIR") or (Path(__file__).resolve().parent / "models"))
MODEL_DIRS: dict[str, str] = {
    "layout": model_contract.LAYOUT_MODEL_DIR_NAME,
    "vl": model_contract.VL_MODEL_DIR_NAME,
}

_CREATE_NO_WINDOW = 0x08000000  # hide the child console (same value as Rust's CREATE_NO_WINDOW)


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
    """PyPI's Linux torch wheel bundles CUDA without the `+cu` tag (metadata looks cpu-only), so the
    installed nvidia-* / triton distributions are the only usable signal."""
    try:
        for dist in importlib.metadata.distributions():
            name = (dist.metadata["Name"] or "").lower()
            if name.startswith("nvidia-") or name == "triton":
                return True
    except Exception:
        return False
    return False


def _torch_build(deps: dict[str, str | None]) -> str | None:
    """torch build variant: +cuNNN suffix = cuda, +cpu = cpu; not installed = None."""
    version = deps.get("torch")
    if version is None:
        return None
    if "+cu" in version:
        return "cuda"
    if "+cpu" in version:
        return "cpu"
    return "cuda" if _has_bundled_cuda() else "cpu"


def _gpu_report() -> dict | None:
    """nvidia-smi probe: no card, no driver, or a failed call all return None (normal, not an error)."""
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
        # The driver's CUDA version is in the nvidia-smi header (first line is the date; CUDA Version is a few lines down)
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
    except Exception as exc:  # a crashed probe still emits JSON and exits 0 (the report is the data)
        # The report must stay complete: a missing field breaks Rust deserialization and hides the real reason
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
