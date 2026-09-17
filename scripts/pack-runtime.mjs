// Pack production resources: relocatable Python 3.12 (bare interpreter + pip),
// pyserver code, and system prompts -> src-tauri/resources/ (gitignored; mapped
// into the install dir by bundle.resources in tauri.conf.json).
//
// Usage: node scripts/pack-runtime.mjs [--force]
//
// Python sources, in order:
//   1. cached archive in src-tauri/resources/.cache/<asset>
//   2. EZPDF_PYTHON_PKG pointing at a local archive (offline / slow download)
//   3. mirror chain: NJU -> ghfast -> GitHub direct (NJU ~1MB/s, direct ~20KB/s)
//
// Deps are not preinstalled; the in-app "install service" fetches them.
// Safety assertion: dist/ must not contain the auth.cfg apiKey (never ship the dev key).

import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const resDir = path.join(root, "src-tauri", "resources");
const cacheDir = path.join(resDir, ".cache");

// Pinned to this astral-sh/python-build-standalone release
const PY_TAG = "20260901";
const PY_VERSION = "3.12.14";

/** Target platform -> python-build-standalone asset triple + interpreter path in the bundle.
 *  Tauri does not cross-compile, so the host platform is always used;
 *  EZPDF_PACK_PLATFORM only drills other platforms locally. */
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
/** Use the stripped variant: unstripped Linux libpython is 209MB and bin/python3.12 98MB
 *  (Windows ships .pdb); stripping only drops debug symbols, verified on both platforms
 *  with an interpreter self-check plus venv creation. */
const PY_VARIANT = "install_only_stripped";
const PY_ASSET = `cpython-${PY_VERSION}+${PY_TAG}-${TARGET.triple}-${PY_VARIANT}.tar.gz`;
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
  // Dedupe hard links: the bundled Python reuses one binary (bin/python3.12, libpython*.so),
  // tar/deb store it once, so the reported size must count it once (else Linux doubles it)
  const seen = new Set();
  for (const entry of fs.readdirSync(p, { withFileTypes: true, recursive: true })) {
    // fs.readdirSync recursive gives relative paths in entry.parentPath
    const abs = path.join(entry.parentPath ?? p, entry.name);
    try {
      const st = fs.statSync(abs);
      if (!st.isFile()) continue;
      const key = `${st.dev}:${st.ino}`;
      if (seen.has(key)) continue;
      seen.add(key);
      total += st.size;
    } catch {
      /* ignore race deletions */
    }
  }
  return total;
}

function fmtSize(bytes) {
  return `${(bytes / 1024 / 1024).toFixed(1)}MB`;
}

// ---- pyserver code / system prompts -------------------------------------------------------

const PY_EXCLUDES = new Set([".venv", "models", "__pycache__", ".pytest_cache", "cache"]);

function packDir(src, dst) {
  rmrf(dst);
  fs.cpSync(src, dst, {
    recursive: true,
    filter: (s) => !PY_EXCLUDES.has(path.basename(s)) && !s.endsWith(".pyc"),
  });
  log(`复制 ${path.relative(root, src)} → ${path.relative(root, dst)}（${fmtSize(dirSize(dst))}）`);
}

// ---- Python runtime ------------------------------------------------------------------------

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

/** Windows ships bsdtar (System32/tar.exe); a PATH tar may be MSYS GNU tar,
 *  which treats `D:\...` as a remote host ("Cannot connect to D:"), so use the system tar */
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

/** Trim unused bulk (tkinter/tcl/test suite/headers/share + Windows debug symbols).
 *  Linux layout checked against the tar manifest: lib/tcl9.0, lib/tk9.0, lib/libtcl9.0.so,
 *  lib/pythonX.Y/config-<triplet>, share/, include/; Windows has tcl/ + DLLs/*.pdb. */
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
  scan("", /\.pdb$/i); // top-level python.pdb / python3.pdb / pythonw.pdb
  scan("DLLs", /\.pdb$/i); // extension module debug symbols (~30MB)
  scan("DLLs", /^(_tkinter|tcl|tk)\d*t?\.(pyd|dll)$/i);
  scan("lib", /^(libtcl|libtk|tcl\d|tk\d|itcl|thread\d|tdbc)/i); // Linux tcl/tk runtime libs and extensions
  scan(path.join("lib", `python${version}`), /^config-/i); // build-time header directory
  scan(path.join("Lib", "venv", "scripts", "nt"), /\.pdb$/i);
}

// ---- safety assertion: dist must not carry the dev key -------------------------------------

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

// ---- main -----------------------------------------------------------------------------------

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
