// 生产资源打包（用户 2026-09-14）：可重定位 Python 3.12（裸解释器 + pip）+
// pyserver 代码 + 系统提示词 → src-tauri/resources/（gitignored；
// tauri.conf.json 的 bundle.resources 把它们映射到安装目录）。
//
// 用法：node scripts/pack-runtime.mjs [--force]
//
// Python 来源（按序）：
//   1. 本地缓存 src-tauri/resources/.cache/<asset>
//   2. EZPDF_PYTHON_PKG 指向的本地压缩包（离线/下载慢时手动放置）
//   3. 镜像链下载：南大镜像 → ghfast → GitHub 直连（实测南大 ~1MB/s，直连 ~20KB/s）
//
// 依赖不预装（用户拍板：裸 Python + pip）；依赖由应用内「一键安装服务」下载。
// 安全断言：dist/ 产物不得含 auth.cfg 的 apiKey（dev 密钥绝不能随包）。

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const resDir = path.join(root, "src-tauri", "resources");
const cacheDir = path.join(resDir, ".cache");

// 与 astral-sh/python-build-standalone 的发布对齐（20260901 发布的最新 3.12）
const PY_TAG = "20260901";
const PY_VERSION = "3.12.14";

/** 目标平台 → python-build-standalone 资产三元组 + 随包解释器相对路径。
 *  打包在哪个平台执行就必须打哪个平台的解释器（Tauri 不做交叉编译），
 *  故直接取 process.platform/arch；EZPDF_PACK_PLATFORM 仅供本地演练其他平台。 */
const PLATFORM = process.env.EZPDF_PACK_PLATFORM ?? process.platform;
const ARCH = process.env.EZPDF_PACK_ARCH ?? process.arch;

function targetFor() {
  if (PLATFORM === "win32" && ARCH === "x64") {
    return { triple: "x86_64-pc-windows-msvc", exe: ["python.exe"] };
  }
  if (PLATFORM === "linux" && ARCH === "x64") {
    return { triple: "x86_64-unknown-linux-gnu", exe: ["bin", "python3"] };
  }
  if (PLATFORM === "linux" && ARCH === "arm64") {
    return { triple: "aarch64-unknown-linux-gnu", exe: ["bin", "python3"] };
  }
  throw new Error(`暂不支持打包 ${PLATFORM}/${ARCH} 的 Python 运行时（Windows x64 / Linux x64|arm64）`);
}

const TARGET = targetFor();
const PY_ASSET = `cpython-${PY_VERSION}+${PY_TAG}-${TARGET.triple}-install_only.tar.gz`;
const ASSET_ENC = encodeURIComponent(PY_ASSET);
const MIRRORS = [
  `https://mirror.nju.edu.cn/github-release/astral-sh/python-build-standalone/${PY_TAG}/${ASSET_ENC}`,
  `https://ghfast.top/https://github.com/astral-sh/python-build-standalone/releases/download/${PY_TAG}/${ASSET_ENC}`,
  `https://github.com/astral-sh/python-build-standalone/releases/download/${PY_TAG}/${ASSET_ENC}`,
];

const force = process.argv.includes("--force");
const stampFile = path.join(resDir, ".runtime-stamp");

function log(msg) {
  console.log(`[pack-runtime] ${msg}`);
}

function rmrf(p) {
  fs.rmSync(p, { recursive: true, force: true });
}

function dirSize(p) {
  let total = 0;
  for (const entry of fs.readdirSync(p, { withFileTypes: true, recursive: true })) {
    // fs.readdirSync recursive gives relative paths in entry.parentPath
    const abs = path.join(entry.parentPath ?? p, entry.name);
    try {
      const st = fs.statSync(abs);
      if (st.isFile()) total += st.size;
    } catch {
      /* 忽略竞态删除 */
    }
  }
  return total;
}

function fmtSize(bytes) {
  return `${(bytes / 1024 / 1024).toFixed(1)}MB`;
}

// ---- pyserver 代码 / 系统提示词 -----------------------------------------------------------

const PY_EXCLUDES = new Set([".venv", "models", "__pycache__", ".pytest_cache", "cache"]);

function packDir(src, dst) {
  rmrf(dst);
  fs.cpSync(src, dst, {
    recursive: true,
    filter: (s) => !PY_EXCLUDES.has(path.basename(s)) && !s.endsWith(".pyc"),
  });
  log(`复制 ${path.relative(root, src)} → ${path.relative(root, dst)}（${fmtSize(dirSize(dst))}）`);
}

// ---- Python 运行时 -------------------------------------------------------------------------

async function download(url, file) {
  const resp = await fetch(url, { redirect: "follow" });
  if (!resp.ok) throw new Error(`HTTP ${resp.status}`);
  const total = Number(resp.headers.get("content-length") ?? 0);
  const tmp = `${file}.part`;
  const out = fs.createWriteStream(tmp);
  let got = 0;
  let lastLog = 0;
  for await (const chunk of resp.body) {
    out.write(chunk);
    got += chunk.length;
    if (Date.now() - lastLog > 3000) {
      lastLog = Date.now();
      const pct = total ? ` ${((got / total) * 100).toFixed(0)}%` : "";
      log(`  下载中 ${fmtSize(got)}${total ? ` / ${fmtSize(total)}` : ""}${pct}`);
    }
  }
  await new Promise((resolve, reject) => {
    out.on("error", reject);
    out.end(resolve);
  });
  fs.renameSync(tmp, file);
}

async function fetchArchive() {
  fs.mkdirSync(cacheDir, { recursive: true });
  const cached = path.join(cacheDir, PY_ASSET);
  if (fs.existsSync(cached) && fs.statSync(cached).size > 1_000_000) {
    log(`使用缓存压缩包 ${path.relative(root, cached)}`);
    return cached;
  }
  const manual = process.env.EZPDF_PYTHON_PKG;
  if (manual) {
    if (!fs.existsSync(manual)) throw new Error(`EZPDF_PYTHON_PKG 不存在: ${manual}`);
    log(`使用 EZPDF_PYTHON_PKG: ${manual}`);
    return manual;
  }
  for (const url of MIRRORS) {
    try {
      log(`尝试下载 Python 运行时：${url}`);
      await download(url, cached);
      log(`下载完成 ${fmtSize(fs.statSync(cached).size)}`);
      return cached;
    } catch (e) {
      log(`  失败（${e.message}），尝试下一个源`);
    }
  }
  throw new Error("所有下载源均失败；可手动下载后设 EZPDF_PYTHON_PKG 指向压缩包");
}

/** Windows 自带 bsdtar（System32/tar.exe）；PATH 上的可能是 MSYS GNU tar，
 *  它会把 `D:\...` 当远程主机（"Cannot connect to D:"），必须显式用系统 tar */
const TAR =
  process.platform === "win32" && fs.existsSync("C:\\Windows\\System32\\tar.exe")
    ? "C:\\Windows\\System32\\tar.exe"
    : "tar";

function extractPython(archive) {
  const stage = path.join(resDir, ".python-extract");
  rmrf(stage);
  fs.mkdirSync(stage, { recursive: true });
  const r = spawnSync(TAR, ["-xzf", archive, "-C", stage], { stdio: "inherit" });
  if (r.status !== 0) throw new Error("tar 解压失败（需要 bsdtar / Windows tar.exe）");
  const extracted = path.join(stage, "python");
  const interpreter = path.join(extracted, ...TARGET.exe);
  if (!fs.existsSync(interpreter)) {
    throw new Error(`解压结果缺少 ${["python", ...TARGET.exe].join("/")}，压缩包结构异常`);
  }
  trimPython(extracted);
  const target = path.join(resDir, "python");
  rmrf(target);
  fs.renameSync(extracted, target);
  rmrf(stage);
  log(`Python 运行时就位：${path.relative(root, target)}（${fmtSize(dirSize(target))}）`);
}

/** 裁剪用不到的大件（tkinter/tcl/测试套件/头文件/share + Windows 调试符号）。
 *  Linux 布局由 tar 清单核对过：lib/tcl9.0/*、lib/tk9.0/*、lib/libtcl9.0.so、
 *  lib/pythonX.Y/config-<triplet>/、share/、include/；Windows 是 tcl/ + DLLs/*.pdb。 */
function trimPython(dir) {
  const [major, minor] = PY_VERSION.split(".");
  const version = `${major}.${minor}`;
  const fixed = [
    "tcl",
    "include",
    "share",
    path.join("Lib", "tkinter"),
    path.join("Lib", "test"),
    path.join("Lib", "idlelib"),
    path.join("lib", `python${version}`, "tkinter"),
    path.join("lib", `python${version}`, "test"),
    path.join("lib", `python${version}`, "idlelib"),
  ];
  for (const rel of fixed) rmrf(path.join(dir, rel));

  const scan = (rel, re) => {
    const target = rel ? path.join(dir, rel) : dir;
    if (!fs.existsSync(target)) return;
    for (const name of fs.readdirSync(target)) {
      if (re.test(name)) rmrf(path.join(target, name));
    }
  };
  scan("", /\.pdb$/i); // 顶层 python.pdb / python3.pdb / pythonw.pdb
  scan("DLLs", /\.pdb$/i); // 扩展模块的调试符号（~30MB）
  scan("DLLs", /^(_tkinter|tcl|tk)\d*t?\.(pyd|dll)$/i);
  scan("lib", /^(libtcl|libtk|tcl\d|tk\d|itcl|thread\d|tdbc)/i); // Linux tcl/tk 运行库与扩展
  scan(path.join("lib", `python${version}`), /^config-/i); // 编译期头文件目录
  scan(path.join("Lib", "venv", "scripts", "nt"), /\.pdb$/i);
}

// ---- 安全断言：dist 不得携带 dev 密钥 -------------------------------------------------------

function assertNoAuthKey() {
  const authCfg = path.join(root, "auth.cfg");
  const dist = path.join(root, "dist");
  if (!fs.existsSync(authCfg) || !fs.existsSync(dist)) return;
  const key = /apiKey\s*:\s*"([^"]+)"/.exec(fs.readFileSync(authCfg, "utf8"))?.[1];
  if (!key) return;
  for (const entry of fs.readdirSync(dist, { withFileTypes: true, recursive: true })) {
    const abs = path.join(entry.parentPath ?? dist, entry.name);
    if (!entry.isFile()) continue;
    if (fs.readFileSync(abs).includes(key)) {
      throw new Error(`dist 泄漏 auth.cfg 的 apiKey（${path.relative(root, abs)}）——构建中止`);
    }
  }
  log("安全断言通过：dist 未包含 auth.cfg 的 apiKey");
}

// ---- main ---------------------------------------------------------------------------------

async function main() {
  fs.mkdirSync(resDir, { recursive: true });
  const stamp = fs.existsSync(stampFile) ? JSON.parse(fs.readFileSync(stampFile, "utf8")) : null;
  const pyOk =
    !force &&
    stamp?.asset === PY_ASSET &&
    fs.existsSync(path.join(resDir, "python", ...TARGET.exe));
  if (pyOk) {
    log(`Python 已就绪（${stamp.asset}），跳过下载（--force 可强制重打）`);
  } else {
    extractPython(await fetchArchive());
  }
  packDir(path.join(root, "pyserver"), path.join(resDir, "pyserver"));
  packDir(path.join(root, "system_prompt"), path.join(resDir, "system_prompt"));
  assertNoAuthKey();
  fs.writeFileSync(
    stampFile,
    JSON.stringify(
      { asset: PY_ASSET, target: TARGET.triple, python: PY_VERSION, packedAt: new Date().toISOString() },
      null,
      2,
    ),
  );
  log("完成：resources/python + resources/pyserver + resources/system_prompt");
}

main().catch((e) => {
  console.error(`[pack-runtime] 失败：${e.message}`);
  process.exit(1);
});
