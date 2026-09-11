"""运行期环境复查：bootstrap.py 是装前 stdlib 探测，这里补装后运行期信息。"""

from __future__ import annotations

from fastapi import APIRouter

router = APIRouter()


@router.get("/env/report")
async def env_report() -> dict:
    report: dict = {
        "torch": None, "torch_build": None,
        "cuda_available": None, "device_name": None,
        "error": None,
    }
    try:
        import torch
        report["torch"] = torch.__version__
        report["torch_build"] = "cuda" if "+cu" in torch.__version__ else "cpu"
        if report["torch_build"] == "cuda":
            report["cuda_available"] = torch.cuda.is_available()
            if report["cuda_available"]:
                report["device_name"] = torch.cuda.get_device_name(0)
    except Exception as exc:
        report["error"] = f"{type(exc).__name__}: {exc}"
    return report
