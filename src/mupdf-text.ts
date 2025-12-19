/**
 * 用 MuPDF 抽出帶字級的結構化文字。
 *
 * 為什麼不走既有的 Rust 路徑：Ghostscript 的 txtwrite 中文正確，但**不輸出字級**，
 * 所以標題層級只能靠行寬猜（見 docs/pdf-to-office.md）。MuPDF 直接給字級。
 *
 * 注意：這是**唯一一個把檔案內容讀進 WebView 的工具**。其他工具的檔案內容都只
 * 存在於 Rust 那側，前端只拿得到路徑。原因是 MuPDF 只有 WASM 版本可用（原生的
 * mupdf-rs 綑綁 C 原始碼、需要 libclang，且 Windows/MSVC 建置被回報不穩定）。
 * 詳見 docs/edit-pdf-analysis.md 第 8.4 節。
 */

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

type MuPdfModule = typeof import("mupdf");
let cached: Promise<MuPdfModule> | null = null;

/**
 * 延遲載入 MuPDF。WASM 有 10 MB，只在真的要用時才付這個代價。
 *
 * 從 /mupdf/ 這個固定路徑載入而不是讓 Vite 打包：MuPDF 用
 * `new URL("mupdf-wasm.wasm", import.meta.url)` 找自己的 WASM，
 * 經過打包後模組路徑會變，WASM 就找不到了。
 */
/** 用變數當 specifier，讓 TypeScript 與 Vite 都不要去解析這個執行期路徑。 */
const MUPDF_ENTRY = "/mupdf/mupdf.js";

function load(): Promise<MuPdfModule> {
  cached ??= import(/* @vite-ignore */ MUPDF_ENTRY) as Promise<MuPdfModule>;
  return cached;
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
  const mupdf = await load();
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
