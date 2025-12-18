# vendor/ghostscript

壓縮引擎。約 40 MB，不進版控，需自行取得。

版本：**Ghostscript 10.07.1**（改版請同步更新此檔並重跑 `bench/compress_bench.py`，
因為壓縮行為會隨版本改變）。

## 取得方式（Windows，不需要管理員權限）

官方只提供 NSIS 安裝器，但它其實是個壓縮包，可以直接解開，不必真的安裝：

```powershell
# 1. 下載安裝器
Invoke-WebRequest 'https://github.com/ArtifexSoftware/ghostpdl-downloads/releases/download/gs10071/gs10071w64.exe' -OutFile "$env:TEMP\gs.exe"

# 2. 取得 7-Zip（msiexec 的 administrative install 不需要管理員權限）
Invoke-WebRequest 'https://github.com/ip7z/7zip/releases/download/26.02/7z2602-x64.msi' -OutFile "$env:TEMP\7z.msi"
msiexec /a "$env:TEMP\7z.msi" /qn TARGETDIR="$env:TEMP\7zx"

# 3. 解開，只留執行時真正需要的部分
$z = "$env:TEMP\7zx\Files\7-Zip\7z.exe"
& $z x "$env:TEMP\gs.exe" "-o$env:TEMP\gs_raw" -y
New-Item -ItemType Directory -Force vendor\ghostscript\bin
Copy-Item "$env:TEMP\gs_raw\bin\gswin64c.exe","$env:TEMP\gs_raw\bin\gsdll64.dll" vendor\ghostscript\bin\
Copy-Item "$env:TEMP\gs_raw\lib","$env:TEMP\gs_raw\Resource","$env:TEMP\gs_raw\iccprofiles" vendor\ghostscript -Recurse

# 4. 驗證
vendor\ghostscript\bin\gswin64c.exe --version   # 應輸出 10.07.1
```

## 為什麼只留這四個目錄

| 目錄 | 大小 | 用途 |
|---|---|---|
| `bin/` | 24 MB | `gswin64c.exe`（主控台版）+ `gsdll64.dll`。GUI 版 `gswin64.exe` 用不到，已排除 |
| `Resource/` | 15 MB | CMap、內建字型、色彩空間定義。少了會導致 CJK 內容輸出破圖 |
| `lib/` | 0.9 MB | PostScript 初始化檔。少了直接無法啟動 |
| `iccprofiles/` | 0.2 MB | 色彩描述檔，CMYK→RGB 轉換需要 |

`doc/`、`examples/`、`uninstgs.exe`、`vcredist_x64.exe` 都已排除。

## 授權

Ghostscript 採 AGPL-3.0 / 商用雙授權。本專案走 AGPL-3.0 路線，因此**整個 LocalPDF 也必須以 AGPL-3.0 散布**，
散布時需一併提供完整原始碼。這是當初選擇 Ghostscript 時就接受的代價——換來的是省下自行實作壓縮引擎的數個月工時。
