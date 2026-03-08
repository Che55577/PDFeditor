# LocalPDF 技術架構與實測基準（Architecture & Benchmarks）

本文件說明 LocalPDF 的底層引擎分工、資料路徑、隱私邊界，以及各項關鍵參數背後的實測基準數據。重現各項數據的腳本位於 `bench/` 目錄。

---

## 一、核心引擎分工與資料流

LocalPDF 依操作性質將 18 項工具分配給五套專職引擎，避免單一引擎「什麼都做、但什麼都做不好」：

| 引擎 | 負責工具 | 技術選型理由 |
| :--- | :--- | :--- |
| **`lopdf` + `subsetter` + `image`（純 Rust）** | 合併、分割、擷取、移除、旋轉、裁切、圖片轉 PDF、浮水印、頁碼、圖章 | 純結構操作僅需搬動 PDF 物件樹（Object Graph）。若走 Ghostscript `pdfwrite` 會整份重寫串流，不僅慢，還會破壞書籤（`/Outlines`）與表單欄位（`/AcroForm`）。JPEG 採 `/DCTDecode` 原生直嵌（零重編碼），PNG 則分離 `/SMask` 灰階透明遮罩。 |
| **Ghostscript 10.07.1** | 壓縮、PDF 轉圖片、修復、Markdown 純文字備援 | PDF 超過 80% 體積來自內嵌點陣圖，唯有雙立方降採樣（Bicubic Downsampling）加 DCT 重新編碼才能大幅縮減體積；修復功能則仰賴 Ghostscript 容錯率極高的剖析器重建損毀的 `xref`。 |
| **MuPDF (WASM)** | 互動式註記編輯器、真塗黑（Redact）、PDF 轉 Markdown | 互動預覽要求單頁算圖低於 50ms（實測 MuPDF WASM 為 **42ms/頁**，而每次啟動 Ghostscript 子行程需 **497ms/頁**，相差 12 倍）。同時 MuPDF 結構化文字能直接提供字級資訊，供 Markdown 推斷 `# / ## / ###` 標題層級。 |
| **qpdf 12.4.2** | AES-256 加密、密碼解密 | 專職處理 PDF 標準加密字典而不重寫頁面內容串流，完整保留書籤與表單；支援 `@-` 由標準輸入讀取參數與密碼，不在作業系統行程清單留痕。 |
| **LibreOffice / `pyext` (`pdf2docx`)** | Office 轉 PDF、PDF 轉 Word | 體積龐大（LibreOffice 約 350 MB、`pyext` 約 216 MB），因此不塞入 25 MB 主安裝檔，採系統偵測或使用者一鍵匯入擴充包（`install_pyext`）的模組化設計。 |

### 確定性管線排序（`Operation::order`）

當使用者同時勾選多項批次功能時，後端 `run_pipeline` 不依點擊先後，而是強制依固定管線順序執行：

1. **入口轉換 / 解密**（`Decrypt`、`OfficeToPdf`、`ImagesToPdf`）
2. **結構修復**（`Repair`）
3. **頁面重組**（`Merge` → `Extract` / `Remove` → `Rotate` → `Crop` → `Split`）
4. **內容疊加**（`Watermark` → `PageNumbers` → `Stamp`）
5. **壓縮**（`Compress`：必須在疊加之後，否則後加的圖片不會被降採樣，且結構重寫會抵銷壓縮效果）
6. **終點加密或匯出**（`Encrypt`、`ToImage`、`ToMarkdown`、`PdfToWord`）

每一步的中間輸出皆由 `Ephemeral` 控制器管理，下一步讀取完畢後立即覆寫為零並刪除。

---

## 二、壓縮引擎參數與實測基準（`src-tauri/src/gs.rs`）

Ghostscript 的三個壓縮等級參數由 `bench/compress_bench.py` 與 `bench/legibility_check.py` 針對三類代表性語料實測決定：

| 壓縮等級 | 彩色/灰階 DPI | 單色 DPI | JPEG 品質 (`JPEGQ`) | 300dpi 掃描件 (32MB) | 照片型錄 (8.8MB) | 純文字論文 (100 頁) |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| **極限壓縮 (`Extreme`)** | 96 | 300 | 50 | **5.8%** (SSIM 0.908) | **4.1%** (SSIM 0.988) | 66% |
| **建議壓縮 (`Recommended`)** | 144 | 600 | 65 | **11.3%** (SSIM 0.944) | **7.0%** (SSIM 0.989) | 66% |
| **低度壓縮 (`Low`)** | 300 | 1200 | 88 | **34.5%** (SSIM 0.986) | **12.9%** (SSIM 0.990) | 66% |

### 關鍵設計決策

1. **為何極限壓縮選 `96 DPI / Q50` 而非 `72 DPI / Q40`**：
   - 在 `72 DPI / Q40` 下，掃描件雖可進一步縮至 3.5% 且整體 SSIM 仍有 0.877，但透過 `bench/legibility_check.py` 放大檢視中文字元時，繁體中文筆畫已黏連模糊至無法辨識。因此以 `96 DPI / Q50` 為下限，確保文字可讀性。
2. **修正 Ghostscript 兩項預設參數陷阱**：
   - `-dColorImageDownsampleThreshold` 預設為 `1.5`（來源解析度必須超過目標的 1.5 倍才觸發降採樣，導致 224 DPI 來源在 150 DPI 目標下完全被跳過）。本專案強制設為 **`1.0`**。
   - `-dPassThroughJPEGImages` 預設為 `true`（未觸發降採樣的 JPEG 會原樣複製，使 `-dJPEGQ` 完全失效）。本專案強制設為 **`false`**，並由整合測試 `gs::tests::every_level_actually_shrinks_files` 守護此不變量。

---

## 三、中文字型子集化與視覺正立座標轉換（`font.rs` / `annotate.rs`）

### 1. CIDFontType2 字型子集化
完整繁體中文字型（如標楷體 4.9 MB、微軟正黑體 20 MB）若直接嵌入會使小檔案暴增數十倍。`src-tauri/src/font.rs` 結合 `ttf-parser` 與 `subsetter`（Typst 專案）：
- 僅抽出浮水印與頁碼實際用到的字元 Glyph，產生 `Type0` + `CIDFontType2`（`Identity-H` 編碼）子集字型（4 個中文字的子集小於原字型 1/20）。
- 自動建立 `/ToUnicode` CMap 串流，確保輸出的浮水印與頁碼文字可被正常搜尋與複製。

### 2. `/Rotate` 與非零原點 `CropBox` 視覺正立座標矩陣
真實世界的 PDF 常帶有頁面旋轉（`/Rotate 90/180/270`）或原點不在 `(0, 0)` 的繼承式 `CropBox`。`pages::visual_page_info` 會先計算出使用者在螢幕上看到的「視覺寬高 `(visual.width, visual.height)`」與 6 參數仿射變換矩陣 `[ma, mb, mc, md, me, mf]`，再由 `annotate.rs` 以：
```text
q {ma} {mb} {mc} {md} {me} {mf} cm ... Q
```
包裹疊加層，確保無論原始頁面如何旋轉或偏移，浮水印、頁碼與圖章永遠落在正確的視覺位置且方向正立。

---

## 四、互動式註記編輯器與真塗黑（`src/mupdf.ts`）

### 1. 為何選用 MuPDF WASM 而非 PDF.js 或原生 `mupdf-rs`
- **不選 PDF.js**：PDF.js 的 `AnnotationEditorLayer` 在處理含 CJK 字元的 `FreeText` 註記時不會產生 `/AP`（Appearance Stream），導致存檔後的中文註記在 Chrome / Edge（PDFium 核心）閱讀器中完全消失。MuPDF WASM 則能正確內嵌 CJK 字型並產生標準 Appearance Stream。
- **不選 `mupdf-rs`（C FFI）**：原生綁定需編譯龐大的 C 原始碼並依賴 `libclang`，在跨平台建置上維護成本高；WASM 版直接取自 npm 預編譯產物，由 `scripts/sync-mupdf.mjs` 自動同步至 `public/mupdf/`。

### 2. 增量儲存（Incremental Save）vs. 塗黑完整重寫（Full Rewrite）
- **一般文字與手繪註記**：採增量寫入，將新註記附加於檔尾，不改動原始位元組。
- **真塗黑（Redaction）**：一旦頁面含有 `Redact` 標記，存檔時會先呼叫 `doc.applyRedactions()` 從內容串流中物理刪除框選範圍內的文字、向量與影像像素，並**強制走完整重寫（Full Save）**，同時在前端驗證輸出位元組絕非原始檔案的前綴延伸，徹底清除歷史物件。

---

## 五、PDF 轉 Word 架構與修正機制（`pyext.rs` / `docxfont.rs` / `scripts/pdf2word.py`）

### 1. 為何採用 `pdf2docx` 獨立擴充包而非 LibreOffice Writer
對同一份 100 頁中文 PDF（`bench/corpus/c1_text.pdf`）的實測對比：

| 指標 | LibreOffice (`writer_pdf_import`) | `pdf2docx` |
| :--- | :--- | :--- |
| 轉換耗時 | 46.3 秒 | **27.6 秒** |
| 輸出 `.docx` 大小 | 255 KB (`document.xml` 膨脹至 **9,937 KB**) | **70 KB** (`document.xml` **1,117 KB**) |
| 段落 `<w:p>` 數量 | 8,700 | **566（真實合併段落）** |
| **浮動文字方塊 `<w:txbxContent>`** | **8,600（佔 98.9%，改一字即破版）** | **0（零文字方塊）** |

在表格偵測基準（`bench/make_tables.py`）上，`pdf2docx` 對有框線、無框線（純對齊）與合併儲存格表格均達 **100%** 準確率；跨頁表格經相鄰同欄數表格接合後亦達 **100%**。

### 2. 兩階段品質修正
1. **向量密集區域（條碼 / QR 碼）前處理（`scripts/pdf2word.py`）**：
   - `pdf2docx` 的資料模型僅處理文字、點陣圖與表格框線，由上百條細小向量路徑組成的條碼與 QR 碼會被直接丟棄。
   - 因此在交給 `pdf2docx` 前，`scripts/pdf2word.py` 會先以 `PyMuPDF` 掃描頁面繪圖路徑，找出高密度細小向量群集（排除跨頁表格框線），將該局部區域以 300 DPI 點陣化為影像疊回頁面，確保條碼與 QR 碼 100% 保留。
2. **中文字型名稱校正（`src-tauri/src/docxfont.rs`）**：
   - `pdf2docx` 常將 PDF 子集字型（如 `AAAAAA+DFKaiShu-SB-Estd-BF`）截斷寫為系統不存在的 `DFKai`，導致 Word 退回預設字型而改變字寬、引發整段重新換行（跑版）。
   - `docxfont::remap` 會掃描本機系統字型家族表，將 `w:eastAsia` 替換為在地化中文名稱（如 `標楷體`）、`w:ascii` 替換為拉丁名稱（如 `DFKai-SB`），大幅提升版面還原度。
