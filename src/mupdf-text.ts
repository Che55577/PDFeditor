/**
 * 用 MuPDF 抽出帶字級的結構化文字。
 *
 * 為什麼不走既有的 Rust 路徑：Ghostscript 的 txtwrite 中文正確，但**不輸出字級**，
 * 所以標題層級只能靠行寬猜（見 docs/pdf-to-office.md）。MuPDF 直接給字級。
 */

import { loadMuPdf, type MuPdf } from "./mupdf";

/**
 * 一行文字，以及它的字級與位置。座標單位是 pt。
 *
 * `block` 是全文件唯一的區塊編號——MuPDF 已經把行分好區塊，那大致就是段落，
 * 比從行寬去猜段落邊界可靠得多。
 */
export interface TextLine {
  text: string;
  size: number;
  x: number;
  y: number;
  block: number;
}

/** MuPDF 的 structured text JSON 形狀，只取我們用得到的部分。 */
interface StJson {
  blocks?: {
    type?: string;
    lines?: { text?: string; font?: { size?: number }; bbox?: { x?: number; y?: number } }[];
  }[];
}

/**
 * 抽出整份文件的文字行，依頁序、再依版面由上而下排列。
 *
 * `bytes` 用完就丟——呼叫端不該保留它。
 */
export async function extractLines(bytes: Uint8Array): Promise<TextLine[]> {
  const mupdf: MuPdf = await loadMuPdf();
  const doc = mupdf.Document.openDocument(bytes, "application/pdf");
  const out: TextLine[] = [];
  // MuPDF 的區塊編號每頁重來，這裡累加成全文件唯一。
  let blockId = 0;
  try {
    const total = doc.countPages();
    for (let i = 0; i < total; i++) {
      const page = doc.loadPage(i);
      const st = JSON.parse(page.toStructuredText("preserve-whitespace").asJSON()) as StJson;
      for (const block of st.blocks ?? []) {
        if (block.type !== "text") continue;
        blockId++;
        for (const line of block.lines ?? []) {
          const text = (line.text ?? "").trim();
          if (!text) continue;
          out.push({
            text,
            size: Math.round((line.font?.size ?? 0) * 100) / 100,
            x: Math.round((line.bbox?.x ?? 0) * 10) / 10,
            y: Math.round((line.bbox?.y ?? 0) * 10) / 10,
            block: blockId,
          });
        }
      }
    }
  } finally {
    doc.destroy?.();
  }
  return out;
}
