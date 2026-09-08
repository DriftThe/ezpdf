/**
 * 骨架期演示数据（仅开发模式）。
 * 布局与 bbox 均按真实格式（PDF 点，612×792 页面）构造，方便阶段2/6 直接复用验证。
 */
import type { Block, Book, BookId, PageInfo, RepoNode } from "../types/domain";

const PAGE_W = 612;
const PAGE_H = 792;

export interface MockLibrary {
  repoRoot: string;
  repoTree: RepoNode[];
  offlineBookIds: BookId[];
  books: Record<BookId, Book>;
}

export function buildMockLibrary(): MockLibrary {
  const root = "C:\\Users\\Drift\\Documents\\ezpdf-repo";
  const offlineRoot = "C:\\Users\\Drift\\AppData\\Roaming\\com.drift.ezpdf\\offline";

  const attention = makeBook(`${root}\\论文\\Attention Is All You Need`, "Attention Is All You Need", 15, 6);
  const diffusion = makeBook(`${root}\\论文\\扩散模型综述`, "扩散模型综述", 8, 8);
  const rust = makeBook(`${root}\\教程\\Rust for Windows`, "Rust for Windows", 24, 2);
  const mixed = makeBook(`${root}\\Mixed Blocks Demo`, "Mixed Blocks Demo", 6, 4);
  const spec = makeBook(`${offlineRoot}\\Spec 心得`, "Spec 心得", 12, 12);
  const scan = makeBook(`${offlineRoot}\\某扫描件`, "某扫描件", 5, 1);

  const books: Record<BookId, Book> = { [attention.id]: attention, [diffusion.id]: diffusion, [rust.id]: rust, [mixed.id]: mixed, [spec.id]: spec, [scan.id]: scan };

  const repoTree: RepoNode[] = [
    {
      type: "folder",
      name: "论文",
      path: `${root}\\论文`,
      children: [
        { type: "book", name: attention.name, path: attention.id },
        { type: "book", name: diffusion.name, path: diffusion.id },
      ],
    },
    {
      type: "folder",
      name: "教程",
      path: `${root}\\教程`,
      children: [{ type: "book", name: rust.name, path: rust.id }],
    },
    { type: "book", name: mixed.name, path: mixed.id },
  ];

  return {
    repoRoot: root,
    repoTree,
    offlineBookIds: [spec.id, scan.id],
    books,
  };
}

function makeBook(dir: string, name: string, pageCount: number, doneCount: number): Book {
  const failedIdx = doneCount >= 4 ? 3 : -1;
  const pages: PageInfo[] = [];
  for (let i = 0; i < pageCount; i++) {
    let status: PageInfo["status"] = "pending";
    if (i < doneCount) status = "done";
    else if (i === doneCount) status = "translating";
    else if (i === doneCount + 1) status = "ocr_done";
    if (i === failedIdx) status = "failed";
    pages.push({ index: i, status, widthPt: PAGE_W, heightPt: PAGE_H, blocks: makeBlocks(i, status) });
  }
  return {
    id: dir,
    name,
    pdfPath: `${dir}\\${name}.pdf`,
    jsonPath: `${dir}\\${name}.json`,
    meta: {
      name,
      pageCount,
      createdAt: "2025-01-01T08:00:00Z",
      updatedAt: "2025-01-02T09:30:00Z",
      targetLang: "zh",
      ocrEngine: "PP-DocLayoutV3 + PaddleOCR-VL-1.6",
      llmModel: "deepseek-chat",
    },
    pages,
  };
}

/** 一页的确定性假块布局：标题/两段正文/图/图注/公式/表格/页脚 */
function makeBlocks(pageIndex: number, status: PageInfo["status"]): Block[] {
  const jitter = (pageIndex % 4) * 6;
  const translated = status === "done";

  return [
    {
      id: `p${pageIndex}-b0`,
      label: "title",
      bboxPt: [72, 56 + jitter, 468, 26],
      score: 0.98,
      source: "Section " + (pageIndex + 1) + ": Attention Mechanisms",
      translation: translated ? `第 ${pageIndex + 1} 节：注意力机制` : null,
    },
    {
      id: `p${pageIndex}-b1`,
      label: "text",
      bboxPt: [72, 100 + jitter, 468, 84],
      score: 0.97,
      source:
        "The dominant sequence transduction models are based on complex recurrent or convolutional neural networks that include an encoder and a decoder.",
      translation: translated ? "【译】主流的序列转换模型基于复杂的循环或卷积神经网络，其中包含编码器与解码器。" : null,
    },
    {
      id: `p${pageIndex}-b2`,
      label: "figure",
      bboxPt: [72, 200 + jitter, 300, 170],
      score: 0.96,
      source: null,
      translation: null,
    },
    {
      id: `p${pageIndex}-b3`,
      label: "figure_caption",
      bboxPt: [72, 378 + jitter, 300, 18],
      score: 0.95,
      source: "Figure 1: Model architecture overview.",
      translation: translated ? "【译】图 1：模型结构总览。" : null,
    },
    {
      id: `p${pageIndex}-b4`,
      label: "formula",
      bboxPt: [396, 210 + jitter, 144, 40],
      score: 0.93,
      source: "$$\\mathrm{Attention}(Q,K,V)=\\mathrm{softmax}(QK^T/\\sqrt{d_k})V$$",
      translation: null,
    },
    {
      id: `p${pageIndex}-b5`,
      label: "table",
      bboxPt: [72, 414 + jitter, 468, 110],
      score: 0.94,
      source: "| Model | BLEU | Cost |\n| --- | --- | --- |\n| Transformer | 28.4 | 3.3e19 |\n| GNMT | 24.6 | 2.3e19 |",
      translation: translated ? "| 模型 | BLEU | 代价 |\n| --- | --- | --- |\n| Transformer | 28.4 | 3.3e19 |\n| GNMT | 24.6 | 2.3e19 |" : null,
    },
    {
      id: `p${pageIndex}-b6`,
      label: "text",
      bboxPt: [72, 540 + jitter, 468, 64],
      score: 0.96,
      source: "In this work we present the Transformer, the first sequence transduction model based entirely on attention.",
      translation: translated ? "【译】本工作提出 Transformer，这是首个完全基于注意力的序列转换模型。" : null,
    },
    {
      id: `p${pageIndex}-b7`,
      label: "footer",
      bboxPt: [72, 736, 468, 18],
      score: 0.99,
      source: `— ${pageIndex + 1} —`,
      translation: translated ? `— ${pageIndex + 1} —` : null,
    },
  ];
}
