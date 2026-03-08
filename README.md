# LocalPDF

[![CI](https://github.com/Che55577/PDFeditor/actions/workflows/ci.yml/badge.svg)](https://github.com/Che55577/PDFeditor/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Che55577/PDFeditor?include_prereleases&label=Release)](https://github.com/Che55577/PDFeditor/releases)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)
![Platform: Windows | macOS | Linux](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)

[English](README.en.md) | **繁體中文**

**LocalPDF** 是一款專為隱私敏感文件設計的**跨平台本機離線 PDF 桌面工具箱**。所有壓縮、合併、轉檔、加解密與塗黑作業皆在你的裝置本機完成——檔案永遠不離開電腦，處理過程產生的中間暫存檔於存檔或關閉視窗時自動以零覆寫並抹除。

---

## 核心特色

* **100% 離線與零網路連線**：程式碼不含任何 HTTP/WebSocket 客戶端相依，並透過系統級 Content-Security-Policy (CSP) 封鎖所有對外連線，適合處理合約、財務報表、醫療與法律等機敏文件。
* **18 合 1 全功能工具箱**：涵蓋日常 PDF 所需的壓縮、頁面重組、格式互轉、密碼保護、浮水印簽章與互動式註記。
* **智慧多工管線串接（Pipeline Chaining）**：可同時勾選多項工具（例如：`旋轉 → 加頁碼 → 浮水印 → 壓縮`），系統會自動依最佳標準順序一次批次執行，中間產物全程受控並在完成後自動抹除。
* **內容串流真塗黑（True Redaction）**：塗黑功能直接從 PDF 底層內容串流移除文字、向量路徑與點陣像素，並強制以完整重寫存檔，杜絕「只蓋黑框卻能被複製出原文」的資安風險。
* **輕量主程式 + 模組化選用擴充**：主安裝檔僅約 25 MB（內建 Ghostscript、qpdf、lopdf 與 MuPDF WASM）；需要「PDF 轉 Word」的使用者可直接在軟體內一鍵匯入離線擴充包（`pyext`）。

---

## 功能總覽（18 項工具）

| 類別 | 工具名稱 | 功能說明 | 底層技術引擎 |
| :--- | :--- | :--- | :--- |
| **壓縮與修復** | **壓縮 PDF** | 提供極限、建議、低度三段式壓縮，兼顧檔案縮減率與中文字體清晰度 | Ghostscript (`pdfwrite`) |
| | **修復 PDF** | 重建損毀的交叉參照表（xref）與物件串流，救回無法開啟的 PDF | Ghostscript |
| **頁面整理** | **合併 PDF** | 將多份 PDF 依拖曳順序合併為單一檔案，保留書籤（Outlines）與表單欄位（AcroForm） | `lopdf`（純 Rust） |
| | **分割 PDF** | 依固定頁數或自訂頁碼區間，將長篇 PDF 拆分為多個獨立檔案 | `lopdf` |
| | **擷取頁面** | 支援 `1, 3, 5-10` 語法，精準抽出指定頁面另存新檔 | `lopdf` |
| | **移除頁面** | 刪除指定頁碼並自動清除孤兒影像物件（`prune_objects`），確實縮減檔案體積 | `lopdf` |
| | **旋轉頁面** | 支援全文件或指定頁面順時針 90° / 180° / 270° 旋轉 | `lopdf` |
| | **裁切白邊** | 自訂上下左右邊界（pt），支援繼承式 `MediaBox` / `CropBox` 精準裁切 | `lopdf` |
| **格式轉換** | **PDF 轉圖片** | 將每一頁輸出為高解析度 JPG 或無損 PNG（20–600 DPI） | Ghostscript |
| | **圖片轉 PDF** | 將多張 JPG（DCT 原生無損直嵌，零重編碼）或 PNG（保留透明通道）合成為 PDF | `lopdf` + `image` |
| | **PDF 轉 Markdown** | 依據字級與段落結構自動推斷 `# / ## / ###` 標題層級並匯出 `.md` | MuPDF WASM / Ghostscript |
| | **PDF 轉 Word** | 重建真實段落與表格為 `.docx`（保留條碼／QR 碼並自動校正中文字型名稱） | `pyext` (`pdf2docx` + `docxfont`) |
| | **Office 轉 PDF** | 支援 Word (`.doc`/`.docx`)、Excel (`.xls`/`.xlsx`)、簡報 (`.ppt`/`.pptx`) 批次轉為 PDF | LibreOffice Headless |
| **安全與遮蔽** | **加密 PDF** | 以 AES-256 為文件加上開啟密碼，密碼透過 `stdin` 傳遞、絕不暴露於系統行程清單 | `qpdf` (`@-`) |
| | **解密 PDF** | 輸入正確密碼後移除 PDF 密碼保護，方便後續歸檔或串接處理 | `qpdf` (`@-`) |
| | **真塗黑（Redact）** | 於預覽畫布框選敏感區域，存檔時徹底從 PDF 結構中抹除該處文字與影像位元組 | MuPDF WASM |
| **標註與疊加** | **加浮水印** | 支援自訂文字、角度、字級、顏色與透明度，自動對中文字型子集化（Subsetting） | `lopdf` + `subsetter` |
| | **加頁碼** | 支援九宮格位置、自訂起始頁碼與 `{n} / {total}` 格式樣板 | `lopdf` + `subsetter` |
| | **蓋圖章／簽名** | 將帶透明背景的 PNG 簽名檔或公司章疊加至指定角落與頁面 | `lopdf` + `/SMask` |
| | **互動式註記** | 視覺化翻頁預覽、新增自由文字標註（FreeText）、手繪筆跡（Ink）與塗黑標記 | MuPDF WASM |

---

## 下載與安裝

請前往 [**GitHub Releases**](https://github.com/Che55577/PDFeditor/releases) 下載對應作業系統的最新安裝檔：

| 作業系統 | 下載檔案格式 | 安裝說明 |
| :--- | :--- | :--- |
| **Windows (x64)** | `LocalPDF_*_x64-setup.exe` | 執行安裝程式即可（內含 Ghostscript 與 qpdf） |
| **macOS (Apple Silicon / Intel)** | `LocalPDF_*.dmg` | 拖入 `Applications` 資料夾（首次開啟請見下方 Gatekeeper 指引） |
| **Linux (Ubuntu / Debian)** | `LocalPDF_*_amd64.deb` / `.AppImage` | `sudo apt install ./LocalPDF_*_amd64.deb`（會自動搭配系統 `ghostscript` 與 `qpdf`） |

### 啟用選用擴充功能

1. **啟用「PDF 轉 Word」（Python 離線擴充包 `pyext`）**：
   - 於 [GitHub Releases](https://github.com/Che55577/PDFeditor/releases) 下載對應平台的擴充包壓縮檔（例如 `localpdf-pyext-Windows-X64.zip` 或 `localpdf-pyext-macOS-ARM64.zip`）。
   - 開啟 LocalPDF，點擊頁尾狀態列或「PDF 轉 Word」面板中的 **「匯入擴充包 (.zip)」** 按鈕，選擇剛下載的 `.zip` 檔即可自動完成驗證與安裝（免管理員權限、免重啟程式）。
2. **啟用「Office 轉 PDF」（LibreOffice）**：
   - LocalPDF 會自動偵測系統已安裝的 LibreOffice。若尚未安裝，可透過以下指令快速安裝：
     ```bash
     # Windows (PowerShell)
     winget install --id TheDocumentFoundation.LibreOffice -e

     # macOS (Homebrew)
     brew install --cask libreoffice

     # Ubuntu / Debian
     sudo apt-get install -y libreoffice
     ```

### 首次啟動安全提示與產物來源驗證（macOS Gatekeeper / Windows SmartScreen）

由於本專案為獨立開源專案，未購買商業憑證機構（Apple Developer ID / Windows EV）簽章，首次開啟時作業系統會顯示未簽章提示：

* **macOS（「LocalPDF 已損毀，無法打開」或「無法驗證開發者」）**：
  將 `LocalPDF.app` 拖入 `/Applications` 後，於終端機執行以下指令移除瀏覽器下載附加的隔離屬性即可正常開啟：
  ```bash
  xattr -cr /Applications/LocalPDF.app
  ```
* **Windows（「Windows 已保護您的電腦」SmartScreen 提示）**：
  點擊視窗上的 **「其他資訊（More info）」** → **「仍要執行（Run anyway）」**。
* **驗證發佈產物完整性與 GitHub 官方建置來源（Provenance Attestation）**：
  每個 Release 皆附帶 `SHA256SUMS.txt` 與 GitHub Actions 產生的 Sigstore 加密建置證明，你可透過 GitHub CLI 驗證安裝包確由本倉庫原始碼自動編譯且未遭竄改：
  ```bash
  gh attestation verify <下載的安裝包檔名> --repo Che55577/PDFeditor
  ```

---

## 隱私與安全保障機制

LocalPDF 的核心設計原則是**「假設所有處理的文件皆為最高機密，且所有外部輸入檔案皆可能包含惡意構造」**：

1. **三道暫存檔抹除防線**：
   - **受控隔離目錄**：所有中間檔僅寫入專屬暫存目錄（亂數檔名、無副檔名，防止作業系統縮圖或搜尋索引服務偷讀）。
   - **即時覆寫刪除（Zero-Fill on Drop）**：無論作業成功、失敗、使用者中途取消或程式發生異常，`Ephemeral` 控制器與 `wipe_dir_all` 皆會先將檔案位元組覆寫為 `0x00` 並強制刷入磁碟（`sync_all`）後才刪除檔案。
   - **啟動自動掃除**：每次啟動 App 時，第一時間自動清掃暫存根目錄，確保即使前次因斷電或強制結束行程也不留殘骸。
2. **密碼零暴露（Process-Table Safe）**：
   - 加解密呼叫 `qpdf @-`，所有參數與密碼皆透過匿名管道（`stdin`）傳入並立即關閉串流，同一台電腦上的其他行程或工作管理員絕對無法從命令列參數偷看到密碼。
3. **子行程沙箱與防死鎖保護**：
   - Ghostscript 全程強制附加 `-dSAFER` 參數封鎖 PostScript 檔案系統存取；`.docx` 字型修正具備 64 MB 單檔與 256 MB 總量之 ZIP 炸彈（Zip Bomb）防護；所有子行程皆具備逾時強殺、雙背景執行緒管道排空與使用者即時取消機制。

> **硬體層誠實聲明**：在現代 SSD 與快閃記憶體上，由於硬體控制器的磨損平衡（Wear-Leveling）與 TRIM 機制，軟體層的零覆寫無法保證實體快閃記憶體區塊立即抹除。若你的威脅模型包含實體硬碟鑑識，請務必搭配作業系統全磁碟加密（Windows BitLocker / macOS FileVault / Linux LUKS）。

---

## 常見問題（FAQ）

<details>
<summary><b>Q1：我可以直接修改 PDF 裡既有的段落文字嗎？</b></summary>

PDF 格式本質上是「帶有絕對座標的字元繪製指令」，檔案內並沒有「段落」或「自動換行欄寬」的資訊。因此：
- 若只需**補充說明、填表、簽名或遮蔽敏感文字**：請直接使用內建的 **「註記」** 工具（提供文字標註、手繪、圖章與真塗黑）。
- 若需要**大幅修改內文並讓段落自動重新排版**：唯一可靠的方式是使用 **「PDF 轉 Word」** 轉為 `.docx` → 在 Word 或 LibreOffice Writer 中編輯 → 再用 **「Office 轉 PDF」** 轉回 PDF。LocalPDF 同時提供這兩端的離線轉換工具。
</details>

<details>
<summary><b>Q2：為什麼「PDF 轉 Word」要做成獨立擴充包，而不直接包進主程式？</b></summary>

高品質的 PDF 轉 Word 需要依賴 Python 生態系的 `pdf2docx`、`PyMuPDF` 與 `OpenCV`（經過瘦身後仍約 216 MB）。將其拆分為一次性匯入的選用擴充包，可讓大多數只需要壓縮、合併、加解密與註記的使用者享受僅 **25 MB** 的極速輕量安裝包。擴充包安裝後同樣 **100% 離線運作**。
</details>

<details>
<summary><b>Q3：用手繪黑筆或深色方塊蓋住身分證字號／帳號，跟「塗黑（Redact）」有什麼不同？</b></summary>

**千萬不要用畫筆或色塊遮蓋機密文字！** 視覺上的色塊只是疊在文字上方的一層圖形，底下的原始文字位元組依然完整留在 PDF 檔案裡，任何人只要按 `Ctrl+F` 或複製貼上就能讀出原文。
請務必使用編輯器中的 **「塗黑（Redact）」** 模式：它會呼叫 `applyRedactions()` 將該區域內的文字、影像與向量路徑從 PDF 內容串流中徹底刪除，並強制重新序列化整份檔案。
</details>

---

## 從原始碼建置與開發

完整的技術架構、引擎分工與壓縮／轉檔實測基準數據，請參閱 [**`docs/ARCHITECTURE.md`**](docs/ARCHITECTURE.md)。

### 環境需求
- **Node.js** 20+（含 `npm`）
- **Rust** stable 工具鏈（`cargo`）
- **Python** 3.11+（用於產生測試語料庫與建置擴充包）
- **Linux 系統套件**（Ubuntu / Debian）：
  ```bash
  sudo apt-get update && sudo apt-get install -y \
    libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf \
    ghostscript qpdf fonts-noto-cjk fonts-wqy-zenhei
  ```

### 快速啟動與測試

```bash
# 1. 安裝前端相依套件（自動同步 MuPDF WASM 至 public/mupdf/）
npm ci

# 2. 下載或編譯外部二進位引擎（含 SHA-256 雜湊校驗）
npm run fetch:vendor

# 3. 執行前端 UI 自動化測試與建置檢查
npm run test:ui
npm run build

# 4. 產生整合測試語料庫並執行 Rust 測試
pip install reportlab pillow
python bench/make_corpus.py
cargo test --manifest-path src-tauri/Cargo.toml

# 5. 啟動桌面開發模式
npm run tauri dev
```

歡迎提交 Issue 與 Pull Request！參與開發前請先閱讀 [**貢獻指南 (CONTRIBUTING.md)**](CONTRIBUTING.md) 與 [**安全政策 (SECURITY.md)**](SECURITY.md)。

---

## 授權條款

本專案採用 [**GNU Affero General Public License v3.0 or later (AGPL-3.0-or-later)**](LICENSE) 授權釋出。隨附之第三方開源元件與二進位引擎授權聲明請見 [**THIRD_PARTY_NOTICES.md**](THIRD_PARTY_NOTICES.md)。
