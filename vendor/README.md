# vendor/

隨程式打包但不進版控的第三方執行檔。Windows 與 macOS 都用同一支腳本重現：

```bash
npm run fetch:vendor
```

| 目錄 | 用途 | 大小 | 授權 |
|---|---|---|---|
| `ghostscript/` | 壓縮、轉圖、修復、文字抽取 | 24 MB | AGPL-3.0 / 商用雙授權 |
| `qpdf/` | 加密與解密 | 8.7 MB | Apache-2.0 |
| `pyext/` | PDF 轉 Word（**獨立擴充包**，不進主安裝檔） | ~216 MB | 各套件自有 |

---

## ghostscript

版本：**10.07.1**（改版請同步更新 `scripts/fetch-vendor.mjs` 並重跑 `bench/compress_bench.py`，壓縮行為會隨版本改變）。

```bash
node scripts/fetch-vendor.mjs ghostscript
```

### 為什麼只留 `bin/`（40.5 MB → 24.0 MB）

官方 Windows 版預設以 `COMPILE_INITS=1` 編譯：`lib/`（PostScript 初始化檔）、`Resource/`（CMap、內建字型）、`iccprofiles/`（色彩描述檔）其實**已經全部編進 `gsdll64.dll` 裡**。

對 8 份測試語料（含 100 頁繁中文字、掃描件、圖文混合、向量條碼、四種表格）實測「完整 4 個目錄」與「只留 `bin/`」：

- `pdfwrite` 壓縮輸出大小一致
- `txtwrite` 中文抽取結果 SHA-256 完全相同
- `png16m` 算圖輸出 SHA-256 完全相同

因此排除外部的 `lib/`、`Resource/`、`iccprofiles/`，以及用不到的 GUI 版 `gswin64.exe`、`doc/`、`examples/`，省下 16.5 MB。

### Windows 與 macOS 的取得方式不同

- **Windows**：官方只提供 NSIS 安裝器（`gs10071w64.exe`），但它本身就是壓縮包，`fetch-vendor.mjs` 會下載 7-Zip 解開並只取出 `bin/gswin64c.exe` 與 `bin/gsdll64.dll`，不必真的安裝。
- **macOS**：Artifex 不提供 macOS 預編二進位檔，而 Homebrew 版動態連結一串 `/opt/homebrew` 底下的 dylib，搬到別台 Mac 就會壞。`scripts/build-gs-macos.sh` 會從官方原始碼包編譯，用內附的 jpeg／png／zlib／freetype／lcms2／openjpeg 靜態連結並開啟 `--enable-compile-inits`，最後用 `otool -L` 驗證只依賴 `/usr/lib` 與 `/System/Library`，產出單一 `vendor/ghostscript/bin/gs`（支援 `ARCH=arm64`、`x86_64` 或 `universal`）。

---

## qpdf

版本：**12.4.2**，Apache-2.0（12.4 起官方提供 macOS 預編版）。

```bash
node scripts/fetch-vendor.mjs qpdf
```

- **Windows**（`qpdf-12.4.2-msvc64.zip`）：攤平放在 `vendor/qpdf/`，保留 `qpdf.exe`、`qpdf30.dll` 與附帶的 MSVC 執行期 DLL（沒裝 Visual C++ Redistributable 的機器也能跑）。排除沒用到的 `zlib-flate.exe` 與 `fix-qdf.exe`。
- **macOS**（`qpdf-12.4.2-bin-macos-{arm64,x86_64}.zip`）：保留 `bin/qpdf` 與 `lib/libqpdf.30.dylib` 兩層結構，因為 `qpdf` 以 `@executable_path/../lib` 尋找函式庫。

---

## pyext（獨立擴充包）

**不進主安裝檔。** 約 216 MB 的 Python 執行期，只有 PDF 轉 Word 用得到，做成獨立擴充包。

```bash
python scripts/build-pyext.py
```

Windows 與 macOS 統一使用 Astral 維護的 `python-build-standalone`（CPython 3.11，可搬移、不綁本機路徑）：安裝 `pdf2docx`，把 `opencv-python` 換成 `opencv-python-headless`（省 70 MB），再移除只用於影片的 ffmpeg 編解碼器（省 29 MB）、Tcl/Tk、C 標頭與 `pip`/`setuptools`。

---

## 授權

Ghostscript 採 AGPL-3.0 / 商用雙授權。本專案走 AGPL-3.0 路線，因此**整個 LocalPDF 也必須以 AGPL-3.0 散布**，散布時需一併提供完整原始碼。這是當初選擇 Ghostscript 時就接受的代價——換來的是省下自行實作壓縮引擎的數個月工時。

qpdf 是 Apache-2.0，與 AGPL 相容。
