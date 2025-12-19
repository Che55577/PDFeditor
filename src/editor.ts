/**
 * 註記編輯器的核心：開檔、算圖、座標映射、寫入註記、存檔。
 *
 * 刻意不碰 DOM——這裡只處理 PDF 與座標，UI 的事情留給 main.ts。
 *
 * ## 為什麼叫「註記」而不是「編輯」
 *
 * 這裡做的是**在原內容之上疊加**，改不了文件裡既有的文字。PDF 沒有段落的概念，
 * 內容串流裡是帶絕對座標的字元串，改一個字要重新量測、重新定位整行、可能重新斷行
 * ——那些資訊在產生 PDF 的當下就已經遺失了。詳見 docs/edit-pdf-analysis.md。
 *
 * ## 座標系
 *
 * MuPDF 的 `page.getBounds()` 與註記的 `setRect()` 用同一個座標系，所以只要算圖與
 * 標註套用同一個縮放，就不必自己處理 PDF 的 Y 軸翻轉。畫布像素與頁面座標之間
 * 只差一個縮放與原點平移。
 */

import { loadMuPdf, type MuPdf } from "./mupdf";

export interface TextStyle {
  size: number;
  /** RGB，各分量 0–1 */
  color: [number, number, number];
}

export interface InkStyle {
  width: number;
  color: [number, number, number];
}

/** 畫布座標（左上原點，像素）。 */
export interface CanvasPoint {
  x: number;
  y: number;
}

export interface PageInfo {
  index: number;
  total: number;
  /** 目前縮放下的畫布尺寸 */
  width: number;
  height: number;
}

/** MuPDF 內建的中文字型後備名稱。標準 14 種字型沒有中文。 */
const CJK_FONT = "Song";

export class Editor {
  #mupdf: MuPdf;
  #doc: any;
  #page: any = null;
  #pageIndex = -1;
  /** 目前頁面的邊界 [x0, y0, x1, y1]，單位 pt */
  #bounds: number[] = [0, 0, 0, 0];
  #scale = 1.5;
  /** 有沒有改過還沒存檔 */
  #dirty = false;

  private constructor(mupdf: MuPdf, doc: any) {
    this.#mupdf = mupdf;
    this.#doc = doc;
  }

  static async open(bytes: Uint8Array): Promise<Editor> {
    const mupdf = await loadMuPdf();
    const doc = mupdf.Document.openDocument(bytes, "application/pdf");
    if (!doc.isPDF?.()) {
      doc.destroy?.();
      throw new Error("這不是有效的 PDF 檔案");
    }
    return new Editor(mupdf, doc);
  }

  get pageCount(): number {
    return this.#doc.countPages();
  }

  get scale(): number {
    return this.#scale;
  }

  get dirty(): boolean {
    return this.#dirty;
  }

  setScale(scale: number) {
    this.#scale = Math.min(4, Math.max(0.25, scale));
  }

  /** 把某一頁畫進畫布，回傳畫布尺寸資訊。 */
  async renderTo(canvas: HTMLCanvasElement, index: number): Promise<PageInfo> {
    const total = this.pageCount;
    const clamped = Math.min(total - 1, Math.max(0, index));
    if (clamped !== this.#pageIndex || this.#page === null) {
      this.#page = this.#doc.loadPage(clamped);
      this.#pageIndex = clamped;
    }
    this.#bounds = this.#page.getBounds() as number[];

    const ctm = this.#mupdf.Matrix.scale(this.#scale, this.#scale);
    // showExtras=true 才會把已存在的註記一起畫出來
    const pix = this.#page.toPixmap(ctm, this.#mupdf.ColorSpace.DeviceRGB, false, true);
    const width = pix.getWidth();
    const height = pix.getHeight();

    canvas.width = width;
    canvas.height = height;
    const ctx = canvas.getContext("2d");
    if (!ctx) throw new Error("無法取得畫布繪圖環境");

    // MuPDF 的 pixmap 是 RGB（每像素 3 byte），canvas 要 RGBA
    const rgb = pix.getPixels() as Uint8Array;
    const rgba = new Uint8ClampedArray(width * height * 4);
    for (let i = 0, j = 0; i < rgb.length; i += 3, j += 4) {
      rgba[j] = rgb[i];
      rgba[j + 1] = rgb[i + 1];
      rgba[j + 2] = rgb[i + 2];
      rgba[j + 3] = 255;
    }
    ctx.putImageData(new ImageData(rgba, width, height), 0, 0);
    pix.destroy?.();

    return { index: clamped, total, width, height };
  }

  /** 畫布像素 → 頁面座標（pt）。 */
  toPage(p: CanvasPoint): [number, number] {
    return [this.#bounds[0] + p.x / this.#scale, this.#bounds[1] + p.y / this.#scale];
  }

  /** 在指定位置加上文字註記。回傳實際佔用的頁面矩形。 */
  addText(at: CanvasPoint, text: string, style: TextStyle): number[] {
    if (!text.trim()) throw new Error("註記文字不能空白");
    const [x, y] = this.toPage(at);

    // 中文是全形約一個字級寬，西文約 0.6。這只是估計——閱讀器會依 /Rect 換行，
    // 所以寧可寬一點：方塊太寬看不出來，太窄會把字切掉。實測 1.25 倍的餘裕夠用。
    const wide = [...text].filter((c) => c.charCodeAt(0) > 0x2e80).length;
    const narrow = [...text].length - wide;
    const width = ((wide + narrow * 0.6) * style.size + style.size) * 1.25;
    const rect = [x, y, x + width, y + style.size * 1.8];

    const annot = this.#page.createAnnotation("FreeText");
    annot.setRect(rect);
    annot.setContents(text);
    annot.setDefaultAppearance(CJK_FONT, style.size, style.color);
    annot.update();
    this.#dirty = true;
    return rect;
  }

  /** 加上手繪筆跡。`strokes` 是多段筆畫，每段是連續的畫布座標。 */
  addInk(strokes: CanvasPoint[][], style: InkStyle): void {
    const usable = strokes.filter((s) => s.length >= 2);
    if (usable.length === 0) throw new Error("沒有可用的筆跡");

    const annot = this.#page.createAnnotation("Ink");
    annot.setInkList(usable.map((s) => s.map((p) => this.toPage(p))));
    annot.setColor(style.color);
    annot.setBorderWidth(style.width);
    annot.update();
    this.#dirty = true;
  }

  /** 目前頁面的註記數量。 */
  annotationCount(): number {
    return this.#page ? this.#page.getAnnotations().length : 0;
  }

  /** 移除目前頁面最後加入的註記。 */
  undoLast(): boolean {
    const annots = this.#page?.getAnnotations() ?? [];
    const last = annots[annots.length - 1];
    if (!last) return false;
    this.#page.deleteAnnotation(last);
    this.#dirty = true;
    return true;
  }

  /**
   * 存成 PDF 位元組。
   *
   * 用增量儲存：新物件附加在原始檔案之後，**原始位元組完整保留**，正好符合
   * 「疊加不動原內容」的原則。
   *
   * ⚠️ 但這也代表**被註記蓋住的內容仍然留在檔案裡**。拿白色方塊蓋住敏感資料
   * 再存檔是不安全的——那需要真正的塗黑（applyRedaction）並完整重寫檔案。
   * 這條規則不能只寫在文件裡，任何未來加入的遮蔽功能都不可以走這個函式。
   */
  save(): Uint8Array {
    const buf = this.#doc.saveToBuffer("incremental");
    const bytes = buf.asUint8Array() as Uint8Array;
    this.#dirty = false;
    return bytes;
  }

  destroy(): void {
    this.#page?.destroy?.();
    this.#doc?.destroy?.();
    this.#page = null;
  }
}
