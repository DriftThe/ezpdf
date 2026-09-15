"""运行期环境复查（占位）。

原 /env/report 路由没有任何调用方：OCR 设置页的三盏灯由 bootstrap.py 的探测报告驱动
（装前），运行期判据还没有需求；且它与 bootstrap 的 torch 构建判定不一致（PyPI 的
Linux torch 轮子带 CUDA 但没有 +cu 标签）。要恢复时一并修这两点。
"""

from __future__ import annotations
