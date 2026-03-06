# LocalPDF

在本機處理 PDF 的工具箱。檔案永遠不離開這台裝置，處理過程中的暫存檔在你存檔或關閉視窗後立即抹除。

目前支援：**壓縮**、**合併**、**擷取頁面**、**移除頁面**、**旋轉**、**裁切**、**分割**、
**轉成圖片**（JPG／PNG）、**圖片轉 PDF**、**修復**、**浮水印**、**頁碼**、**圖章**、
**Office 轉 PDF**、**轉成 Markdown**、**註記**（互動式編輯器）。

**PDF 轉 Word** 需要另外安裝擴充包（見下節）。不做 PDF 轉 PowerPoint——沒有開源方案做得好。

### 想改 PDF 的內文？

**改不了。** PDF 沒有段落的概念，內容串流裡是帶絕對座標的字元串；改一個字要重新量測、
重新定位整行、可能重新斷行——那些資訊在產生 PDF 的當下就已經遺失。編輯器提供的是
**疊加式註記**，不是改動原文。

實際可行的路只有一條：**PDF 轉 Word → 在 Word 裡編輯 → 用「Office 轉 PDF」轉回來**。
兩端本專案都有，但中間的編輯要在 Word（或 LibreOffice Writer）裡做。

### PDF 轉 Word 的字型修正

pdf2docx 把 PDF 的字型名稱直接寫進 docx，但它的正規化會產生**系統上不存在的名稱**：
實測來源用 `AAAAAA+DFKaiShu-SB-Estd-BF`（標楷體子集），輸出寫成 `DFKai`——Windows 上
沒有這個字型，Word 與 LibreOffice 只好退回預設字型。結果是中文換成另一套字（看起來像
變粗，其實是替代字型本身較重），而且字寬一變整段就重新換行，也就是「跑版」。

`docxfont` 掃描系統字型的家族名稱，把對不上的換成最接近的一個。兩個細節：

* `w:eastAsia` 用**中文名稱**（標楷體），`w:ascii` 用拉丁名（DFKai-SB）。實測這樣的版面
  比兩者都用拉丁名準確得多——`w:eastAsia` 本來就是給 CJK 字型的欄位。
* 比對要能處理 PDF 的子集前綴（`ABCDEF+`）與截斷（`DFKai` → `DFKai-SB`）。

**還是會有差異。** 來回一趟後行首斷點不可能完全一致：pdf2docx 正確地把 PDF 的硬換行
合併成段落，Word 再依自己的欄寬重新斷行。這是格式轉換的本質，不是可以修掉的缺陷。

### 加密與解密

用 qpdf（Apache-2.0，隨程式打包 8 MB），AES-256。

**密碼不放在命令列。** 命令列參數在行程清單裡是公開的，同一台機器的其他使用者用工作
管理員就看得到。所以走 qpdf 的 `@-`：參數從 stdin 一行一個讀進去，密碼既不進行程清單
也不落地。

解密**需要使用者提供正確密碼**，本程式不做任何密碼破解。

### Python 擴充包

PDF 轉 Word 只有 Python 生態有堪用的實作。實測比較見
[`docs/pdf-to-office.md`](docs/pdf-to-office.md)——LibreOffice 產出的是每個視覺行一個
浮動文字方塊（改一個字整段版面就散），pdf2docx 產出真段落、零文字方塊。

代價是 216 MB 的 Python 執行期，所以做成獨立擴充包，主安裝檔維持 25 MB。建立方式：

```
python scripts/build-pyext.py
```

它會下載可嵌入版 Python、裝好 pdf2docx，並把用不到的東西清掉（完整的 opencv 換成
headless 版省 70 MB、移除只用於影片的 ffmpeg 編解碼器省 29 MB、裝完的 pip 與 setuptools
再省 8 MB）。

擴充包是**一次性下載安裝**，不是執行期連網取件——「處理過程不連網」的宣稱不受影響。

## 為什麼是這個順序

同類線上服務（iLovePDF、Smallpdf）最常被使用的功能是壓縮，而壓縮也正好是最不該把檔案上傳出去的操作——
需要壓縮的通常是掃描件、合約、報告這類完整內容的文件。所以第一版只做壓縮，把隱私架構打穩，再往外長。

## 功能可以複選並串接

介面是勾選式的：勾幾個功能，就會依**合理的處理順序**依序跑完，中間產物不會落到使用者手上
（它們是受管理的暫存檔，下一步算完就抹除）。

順序不是照勾選的先後，而是寫死在 `Operation::order`：格式轉換要先把來源變成 PDF、修復要在
動結構之前、頁面操作要在加註記之前（否則註記會蓋在被刪掉的頁上）、**壓縮幾乎一定要最後**
——先壓再疊圖只會讓疊上去的東西沒被壓到，而且中間步驟重寫檔案會抵銷壓縮效果。

執行按鈕會直接顯示實際順序（例如「依序執行 3 項：旋轉 → 頁碼 → 壓縮」），設定面板也用 CSS
`order` 照同樣順序排列，畫面上看到的順序就是實際處理的順序。

互相衝突的組合在勾選當下就擋掉，不會等到執行才報錯：擷取與移除頁面、兩個入口轉換工具
（來源格式不同）、兩個終點工具（都產生非 PDF），以及註記（互動式，不能和批次功能混用）。

## 兩套引擎，各司其職

| | 用途 | 為什麼 |
|---|---|---|
| **Ghostscript** | 壓縮、轉成圖片、修復 | 檔案大小八成以上來自內嵌影像，真正有效的壓縮是重新採樣＋重新編碼影像；出圖需要完整的算圖器；修復則是靠它容錯性極高的剖析器重建結構 |
| **lopdf**（純 Rust） | 合併／選頁／旋轉／裁切／分割／JPG 轉 PDF | 這些只需要搬動物件結構。走 Ghostscript 會整份重寫文件——慢，而且會丟掉書籤、連結與表單欄位 |

圖片轉 PDF 走 lopdf 而非 Ghostscript，是因為 JPEG 的 DCT 壓縮本來就是 PDF 原生支援的串流過濾器：
原始位元組可以直接當成 `/DCTDecode` 影像塞進去，**不解碼也不重新編碼**，完全無損。

PNG 沒有這個待遇，必須解碼後重新以 Flate 編碼。但它換來的是**透明度**——簽名檔與公司章幾乎
都帶透明背景，沒有它蓋出來會是一個白方塊糊住文件。透明度在 PDF 裡是獨立的灰階影像，
掛在影像的 `/SMask` 上。

### 兩個工具會把檔案內容讀進 WebView

其餘工具的檔案內容都只存在於 Rust 那側，前端拿得到的僅有路徑。例外是**轉成 Markdown**
與**註記編輯器**，兩者都需要 **MuPDF（WASM）**：

- Markdown 需要字級才能判斷標題層級。Ghostscript 的 `txtwrite` 中文正確但不輸出字級。
- 編輯器需要在畫面上算圖。實測 Ghostscript 每頁 497 ms、MuPDF 42 ms，差 12 倍——
  換頁等半秒的編輯器等同壞掉。

不用原生的 `mupdf-rs`：它綑綁 C 原始碼、需要 libclang，且 Windows/MSVC 建置被回報不穩定。
完整理由見 [`docs/edit-pdf-analysis.md`](docs/edit-pdf-analysis.md) 第 8.4 節。

MuPDF 載入失敗時會自動退回 Ghostscript 那條路，標題層級較不準但仍可用。

`public/mupdf/` 的檔案是從 `node_modules/mupdf/dist/` 複製過來的，刻意不讓 Vite 打包——
MuPDF 用 `new URL("mupdf-wasm.wasm", import.meta.url)` 找自己的 WASM，經過打包後模組路徑
會變，WASM 就找不到了。

### LibreOffice 是唯一不隨程式打包的引擎

完整安裝約 350 MB，塞進安裝檔會讓體積從 20 MB 變成十幾倍，所以改為偵測系統上既有的安裝，
沒裝就在界面上明確標示該功能不可用。要用 Office 轉檔請自行安裝：

```powershell
# Windows
winget install --id TheDocumentFoundation.LibreOffice -e

# macOS
brew install --cask libreoffice
```

**只做 Office → PDF 這個方向。** 反方向（PDF → Word/Excel）LibreOffice 也做得到，但它是透過
Draw 模組匯入，產出的是一堆文字方塊拼成的版面，實務上不能用——與其做一個爛的，不如不做。

呼叫的是 `soffice.com` 而不是 `soffice.exe`：在 Windows 上 `.exe` 是視窗版啟動器，下完命令會
立刻返回，我們會在轉檔還沒完成時就去讀輸出檔。`.com` 是主控台版，會確實等到轉檔結束。

### 浮水印與頁碼：字型子集化

中文要嵌進 PDF 需要 Type0 複合字型（CIDFontType2 後代 ＋ Identity-H 編碼），而且**必須子集化**
——完整內嵌一份標楷體是 4.9 MB、微軟正黑體是 20 MB，每個輸出檔都背這個重量不可行。
子集化用 `subsetter`（Typst 專案）處理，實測四個字的子集小於原始字型檔的 1/20。

另外一定要產生 ToUnicode CMap，否則從輸出的 PDF 複製文字出來會是亂碼。

疊加時**原內容一個位元組都不動**，只在內容串流陣列的前後各加一段：前面補 `q`，後面補 `Q` 加上
我們的繪圖指令。這樣即使原內容留下不平衡的繪圖狀態，疊加層仍然從乾淨的狀態開始。

lopdf 沒有 C 相依，之後移植 Android 時這一半的功能可以直接帶過去。

轉成 JPG 刻意一頁呼叫一次 Ghostscript，而不是用它的 `%03d` 檔名樣板一次輸出整份：
樣板會把檔案寫到暫存管理之外的名字上，等於在隱私架構旁邊多開一條沒人看管的路徑。
代價是每頁多一次行程啟動，換來每個位元組都落在受管理的暫存檔裡。

## 隱私設計

- **不連網**。程式碼中沒有任何 HTTP client 依賴。
- **暫存範圍受控**。所有落地的中間檔案只出現在受管理的暫存目錄（Windows：`%LOCALAPPDATA%\com.localpdf.app\eph\`；macOS：`~/Library/Application Support/com.localpdf.app/eph/`），亂數命名、無副檔名
  （避免檔案總管／Finder 與索引服務去讀取內容產生縮圖）。
- **三道抹除保險**：
  1. `Ephemeral` 的 `Drop` 實作——處理失敗、使用者取消、甚至程式 panic，暫存檔都會被抹除
  2. 存檔時同磁碟用 `rename`（原子操作，不留第二份內容），跨磁碟則複製後立刻抹除來源
  3. 每次啟動先清空暫存根目錄，處理掉當機留下的殘骸
- **Ghostscript 以 `-dSAFER` 執行**，限制 PostScript 存取檔案系統。處理來路不明的 PDF 時這是必要防線。

### 塗黑是真的刪除，不是畫黑框

編輯器的「塗黑」用 MuPDF 的 `applyRedactions()`，**從內容串流真的移除**文字、影像與線條圖形，
而不是在上面蓋一層黑色。

有塗黑標記時，存檔**強制走完整重寫**而不是增量寫入。這一點不是可有可無的細節：增量儲存會把
原始位元組原封不動留在檔案開頭，被「塗掉」的文字用抽取工具一撈就回來，等於沒塗。

驗證方式（`src/editor.ts` 的 `save()`）：塗掉第一頁標題後，除了確認文字從內容中消失，還要確認
**輸出不是原始檔的前綴延伸**——是的話就代表走了增量儲存，那是不安全的。

其他任何遮蔽功能都必須走這條路徑，不可以自己呼叫 `saveToBuffer("incremental")`。

### 註記編輯器：疊加，不是編輯

編輯器做的是**在原內容之上疊加**，改不了文件裡既有的文字。PDF 沒有段落的概念，內容串流裡
是帶絕對座標的字元串，改一個字要重新量測、重新定位整行、可能重新斷行——那些資訊在產生 PDF
的當下就已經遺失。所以它叫「註記」而不是「編輯」，界面上也這樣寫。

存檔用 PDF 的增量寫入：新物件附加在原始檔案之後，**原始位元組完整保留**。

⚠️ **這也代表用色塊蓋住敏感資料並不安全**——被蓋住的內容仍然完整躺在檔案裡，用文字抽取
工具一撈就出來。要遮蔽請用「塗黑」（見上一節），不要用手繪或圖章畫黑框。

### 已知的次要表面：系統 WebView profile

Tauri 在 Windows 使用系統的 WebView2（`%LOCALAPPDATA%\com.localpdf.app\EBWebView\`），在 macOS 使用系統的 WKWebView（`~/Library/WebKit/com.localpdf.app/`），
兩者都會建立一份瀏覽器 profile 目錄（`Cookies`、`History` 等）。本程式只載入內嵌的
本機頁面、不做任何網路導覽，所以這些檔案實際上都是空殼，**不會包含你的 PDF 內容**。

但對一個宣稱資料不外傳的工具而言，這仍是應該講明的表面。待辦：評估把 WebView 的
user data folder 指向暫存位置並在結束時一併清除。

### 誠實聲明

在 SSD 與快閃記憶體上，因為 wear-leveling 與 TRIM，**「覆寫刪除」無法保證原始資料真的被抹除**。
本程式的覆寫是盡力而為。真正的保障是：暫存存在時間極短、範圍受控、且全程不連網。
若你的威脅模型需要更高保障，請搭配全磁碟加密（Windows：BitLocker；macOS：FileVault）。

## 開發

需要 Node 20+、Rust stable，以及平台的 C 編譯工具（Windows：MSVC Build Tools；macOS：`xcode-select --install`）。

```bash
npm install
npm run fetch:vendor   # 取得 Ghostscript（24 MB）與 qpdf（8.7 MB），見 vendor/README.md
npm run tauri dev
```

⚠️ **不要直接執行 `target/debug/localpdf.exe`。** debug 建置會去載入 `tauri.conf.json` 裡的
`devUrl`（`http://localhost:1420`）取得前端，沒開 Vite 開發伺服器就只會看到
`ERR_CONNECTION_REFUSED`。要獨立執行請用 `npm run tauri build` 產生的 release 版，
那才會把前端資產內嵌進執行檔。

在 macOS 上打包後，請執行 `bash scripts/sign-macos.sh` 對 `.app` 內嵌的 `gs`、`qpdf` 與擴充包做由內而外的完整簽章（未設定 `APPLE_SIGNING_IDENTITY` 時使用 ad-hoc 簽章供本機測試，設定後啟用 Hardened Runtime 供公證發佈）。

## 壓縮參數怎麼定的

`src-tauri/src/gs.rs` 裡三個壓縮等級的參數不是拍腦袋決定的，是 `bench/compress_bench.py` 實測出來的。
該腳本會對每組參數量測「輸出大小 / 耗時 / SSIM 畫質分數」，結果寫進 `bench/results.csv`。

**要改壓縮參數，請附上新的實測數據。**

```
python bench/make_corpus.py        # 產生測試語料
python bench/compress_bench.py     # 大小 / 耗時 / SSIM 基準測試
python bench/legibility_check.py   # 中文可讀性目視對照
```

`legibility_check.py` 不是多餘的：SSIM 量的是像素相似度，量不出「這行中文還讀不讀得懂」。
極限壓縮原本要選 72dpi/q40（掃描件可壓到 3.5%、SSIM 仍有 0.877），但目視對照顯示中文筆畫已經
黏連到難以辨識——那是功能失效而非畫質下降。最後改用 96dpi/q50，多 0.75 MB 換回可讀性。
**調整壓縮參數時，SSIM 與目視對照要一起看。**

實測結果（Ghostscript 10.07.1，v0.1 基準）：

| 等級 | 300dpi 掃描件 32MB | 照片型錄 8.8MB | 純文字 100 頁 |
|---|---|---|---|
| 極限壓縮 | 5.8%（SSIM 0.908） | 4.1%（0.988） | 66% |
| 建議壓縮 | 11.3%（0.944） | 7.0%（0.989） | 66% |
| 低度壓縮 | 34.5%（0.986） | 12.9%（0.990） | 66% |

### 踩到的坑：Ghostscript 兩個預設值會讓壓縮完全失效

第一輪實測出現了荒謬的結果——「低度壓縮」對 300dpi 掃描件輸出 **100.0%**（完全沒變小），
「建議壓縮」對照片型錄輸出 **80%**。原因是兩個預設值交互作用：

- `-dColorImageDownsampleThreshold` 預設 **1.5**：來源解析度必須超過目標的 1.5 倍才降採樣。
  300dpi 來源、300dpi 目標 → 不處理。224dpi 來源、150dpi 目標（門檻 225）→ 差 1dpi，也不處理。
- `-dPassThroughJPEGImages` 預設 **true**：沒被降採樣的 JPEG 原樣複製，`-dJPEGQ` 完全不生效。

兩者合起來就是使用者抱怨「壓縮完檔案大小沒變」的根源。修正方式是把 threshold 設為 `1.0`
並關閉 passthrough，讓「只要超過目標解析度就處理」成為一致行為。修正後同一組參數
對掃描件從 100% 降到 34.5%。

純文字 PDF 固定約 66%（來自字型子集化與物件流壓縮），與壓縮等級無關——這是正常的，
文字 PDF 本來就沒有影像可以降採樣。

## 授權

AGPL-3.0。本專案內嵌 Ghostscript（AGPL），因此整體必須以 AGPL 散布。
