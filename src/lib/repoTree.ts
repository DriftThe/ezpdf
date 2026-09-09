/**
 * 把后端 ts-rs 绑定的平铺仓库索引（RepoTree）转换为 UI 侧的递归 RepoNode 树。
 *
 * 约定（平铺式索引管理，v1 不支持子文件夹；树只是索引，pdf 实体后续凭索引向后端请求）：
 * - RepoTree.folders: 顶层目录名列表（平铺，可含没有书的空目录）
 * - PDFStruct.name:   书名（节点显示名；书目录 = belong 目录 + name，符合 "<书名>/<书名>.pdf" 约定）
 * - PDFStruct.bind:   绑定的 ezpdf 结构 JSON 的仓库相对路径；null = 尚未解析
 * - PDFStruct.belong: 所属顶层目录名；null/空 = 根级
 * 书的唯一索引 = belong 目录 + name（即书目录的绝对路径）。
 * RepoNode 保留递归结构（folder.children），以便将来恢复子文件夹支持时前端无需再改。
 */
import type { RepoNode, RepoTree } from "../types/domain";

interface FolderAcc {
  name: string;
  /** 相对仓库根的路径（Windows 分隔符），用作节点 key */
  relPath: string;
  children: Map<string, FolderAcc>;
  /** 挂在该文件夹下的书，按书目录绝对路径去重 */
  books: Map<string, { name: string; absPath: string; bind: string | null }>;
}

export function buildRepoNodes(repoRoot: string, tree: RepoTree): RepoNode[] {
  const root: FolderAcc = { name: "", relPath: "", children: new Map(), books: new Map() };

  /** 顶层目录；name 兼容容忍 "/"（将来恢复子文件夹时无需改这里） */
  function ensureFolder(name: string): FolderAcc {
    const parts = name.split("/").filter(Boolean);
    let cur = root;
    let acc = "";
    for (const part of parts) {
      acc = acc ? `${acc}\\${part}` : part;
      let next = cur.children.get(part);
      if (!next) {
        next = { name: part, relPath: acc, children: new Map(), books: new Map() };
        cur.children.set(part, next);
      }
      cur = next;
    }
    return cur;
  }

  // 显式登记的（空）目录
  for (const name of tree.folders ?? []) {
    if (name) ensureFolder(name);
  }

  // 书：定位 = belong（单层目录名，null/空 = 根级）；书目录 = belong + name
  for (const pdf of tree.pdfs ?? []) {
    if (!pdf.name) continue;
    const folder = pdf.belong ? ensureFolder(pdf.belong) : root;
    const relDir = pdf.belong ? `${toWinSeg(pdf.belong)}\\${pdf.name}` : pdf.name;
    const absPath = `${repoRoot}\\${relDir}`;
    folder.books.set(absPath, { name: pdf.name, absPath, bind: pdf.bind ?? null });
  }

  return toNodes(root);

  function toWinSeg(seg: string): string {
    return seg.split("/").filter(Boolean).join("\\");
  }

  function toNodes(acc: FolderAcc): RepoNode[] {
    const nodes: RepoNode[] = [];
    const folders = [...acc.children.values()].sort((a, b) => a.name.localeCompare(b.name, "zh"));
    for (const f of folders) {
      nodes.push({ type: "folder", name: f.name, path: f.relPath, children: toNodes(f) });
    }
    const books = [...acc.books.values()].sort((a, b) => a.name.localeCompare(b.name, "zh"));
    for (const b of books) {
      nodes.push({ type: "book", name: b.name, path: b.absPath, bind: b.bind });
    }
    return nodes;
  }
}
