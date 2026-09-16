# ezpdf 解析服务镜像（CPU / GPU 两种，靠 VARIANT 构建参数切换）
#
#   docker build -t ezpdf-pyserver:cpu --build-arg VARIANT=cpu pyserver
#   docker build -t ezpdf-pyserver:gpu --build-arg VARIANT=gpu pyserver
#
# 模型（models/，~1.9GB）直接 COPY 进镜像 → 自包含快照，容器起来即可推理，不依赖联网。
# torch 的 CUDA 依赖由 pip 的 nvidia-* 轮子自带（宿主只需装好驱动 + nvidia-container-toolkit），
# 因此 GPU 变体也基于 python:3.12-slim，不需要 nvidia/cuda 基础镜像。
# 基础镜像可换（Docker Hub 在部分地区不可达）：
#   --build-arg BASE_IMAGE=docker.m.daocloud.io/library/python:3.12-slim
ARG BASE_IMAGE=python:3.12-slim
FROM ${BASE_IMAGE}

ARG VARIANT=cpu
# 国内构建可用镜像源（与客户端「使用镜像源」同源）：
#   --build-arg APT_MIRROR=mirrors.tuna.tsinghua.edu.cn
#   --build-arg PIP_INDEX=https://pypi.tuna.tsinghua.edu.cn/simple
#   --build-arg TORCH_INDEX=https://mirror.sjtu.edu.cn/pytorch-wheels/cpu   # GPU 变体用 .../cu132
ARG APT_MIRROR=""
ARG PIP_INDEX=""
ARG TORCH_INDEX=""

ENV PYTHONUNBUFFERED=1 \
    PIP_DISABLE_PIP_VERSION_CHECK=1 \
    EZPDF_HOST=0.0.0.0 \
    EZPDF_PORT=9055 \
    EZPDF_MODELS_DIR=/app/models \
    EZPDF_TOKEN_FILE=/data/token.txt \
    EZPDF_MAX_BATCH_PAGES=32

# 服务端只用 opencv 做数组运算/解码（见 app/services/pipeline.py），不需要 GUI：
# 所以下面装的是 **headless** 轮子，也就不用 libgl1——省掉它拖来的 mesa/libllvm（约 200MB）。
# 可选换 apt 源（deb.debian.org 在部分地区会 502）
# apt 缓存挂 BuildKit cache mount（列表/包都在缓存里，不进镜像层）：
# 改了这层重跑也只从缓存解包，不会再下一遍 deb
RUN --mount=type=cache,target=/var/cache/apt,sharing=locked \
    --mount=type=cache,target=/var/lib/apt,sharing=locked \
    set -eux; \
    if [ -n "$APT_MIRROR" ]; then \
        for f in /etc/apt/sources.list.d/debian.sources /etc/apt/sources.list; do \
            if [ -f "$f" ]; then sed -i "s|deb.debian.org|$APT_MIRROR|g; s|security.debian.org|$APT_MIRROR|g" "$f"; fi; \
        done; \
    fi; \
    apt-get update -o Acquire::Retries=3; \
    apt-get install -y --no-install-recommends libglib2.0-0 curl

WORKDIR /app

COPY requirements.txt requirements-torch-cpu.txt requirements-torch-cuda.txt ./
# pip 缓存同样挂 cache mount：改这层重跑时 torch 那几个大轮子直接从缓存装，
# 不再重新下载（--no-cache-dir 会把这个缓存废掉，所以这里不传）
RUN --mount=type=cache,target=/root/.cache/pip \
    set -eux; \
    extra=""; [ -n "$PIP_INDEX" ] && extra="--extra-index-url $PIP_INDEX"; \
    pip install -r requirements.txt $extra; \
    # headless 版：去掉 libGL（GUI）依赖，服务端不需要 imshow/highgui
    pip uninstall -y opencv-contrib-python; \
    pip install "opencv-contrib-python-headless==4.10.0.84" $extra; \
    if [ "$VARIANT" = "gpu" ]; then torch_index="https://download.pytorch.org/whl/cu132"; \
    else torch_index="https://download.pytorch.org/whl/cpu"; fi; \
    # Linux 的 PyPI torch 轮子默认捆绑 CUDA（~5GB），CPU 变体必须走 /cpu 索引
    [ -n "$TORCH_INDEX" ] && torch_index="$TORCH_INDEX"; \
    if [ "$VARIANT" = "gpu" ]; then \
        pip install -r requirements-torch-cuda.txt --index-url "$torch_index" $extra; \
        # 只卸 triton（691MB，torch.compile/inductor 的代码生成器）：pipeline.py 全是 eager
        # no_grad 前向，不用 compile。**必须与安装同一个 RUN**——卸载写在单独一层里，
        # 被删的文件仍然占着上面那层。
        # 注意 nvidia-nccl/cusparselt/nvshmem/cupti 看着像"只有多卡/稀疏才用"，其实
        # libtorch_cuda 等 .so 直接链接它们（ldd 全是 not found），卸了 torch 直接 import 失败
        pip uninstall -y triton; \
    else \
        pip install -r requirements-torch-cpu.txt --index-url "$torch_index" $extra; \
    fi

COPY app ./app
COPY bootstrap.py ./
COPY models ./models

# token.txt 落在 /data（compose 挂卷）——重建容器不换令牌，客户端不用重新粘贴
RUN mkdir -p /data

EXPOSE 9055

# 健康检查要带 token：从令牌文件读（/health 也在校验范围内）
HEALTHCHECK --interval=30s --timeout=5s --start-period=90s --retries=3 \
    CMD curl -fsS -H "x-ezpdf-token: $(cat $EZPDF_TOKEN_FILE)" "http://127.0.0.1:${EZPDF_PORT}/health" || exit 1

CMD ["python", "-m", "app.server_docker"]
