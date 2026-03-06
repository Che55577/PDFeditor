//! Ghostscript 壓縮引擎的封裝。
//!
//! 為什麼用 Ghostscript：PDF 檔案大小有八成以上來自內嵌影像，真正有效的壓縮
//! 是「重新採樣 + 重新編碼影像」。純結構層的重寫（qpdf / lopdf）通常只省 5~15%。
//! 專案採 AGPL-3.0，因此可以合法內嵌 Ghostscript。

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    /// 極限壓縮：檔案最小，畫質明顯下降。適合純閱讀、上傳附件。
    Extreme,
    /// 建議壓縮：預設值。螢幕上幾乎看不出差異。
    Recommended,
    /// 低度壓縮：保留列印品質。
    Low,
}

impl Level {
    /// 影像參數。數值由 `bench/compress_bench.py` 實測後決定，改動請附上新數據。
    ///
    /// v0.1 基準 實測（Ghostscript 10.07.1，數字為輸出/原始大小，括號為 SSIM）：
    ///
    /// |            | 300dpi 掃描件 32MB | 照片型錄 8.8MB | 純文字 |
    /// |------------|--------------------|----------------|--------|
    /// | Extreme    | 5.8% (0.908)       | 4.1% (0.988)   | 66%    |
    /// | Recommended| 11.3% (0.944)      | 7.0% (0.989)   | 66%    |
    /// | Low        | 34.5% (0.986)      | 12.9% (0.990)  | 66%    |
    ///
    /// Extreme 沒有選更激進的 72dpi/q40（可再小到 3.5%）：那個設定下掃描的中文字
    /// 已經糊到難以辨識，是功能失效而不只是畫質下降。
    fn args(self) -> Vec<String> {
        let (color, gray, mono, q) = match self {
            Level::Extreme => (96, 96, 300, 50),
            Level::Recommended => (144, 144, 600, 65),
            Level::Low => (300, 300, 1200, 88),
        };
        [
            "-dDownsampleColorImages=true",
            &format!("-dColorImageResolution={color}"),
            "-dColorImageDownsampleType=/Bicubic",
            "-dDownsampleGrayImages=true",
            &format!("-dGrayImageResolution={gray}"),
            "-dGrayImageDownsampleType=/Bicubic",
            "-dDownsampleMonoImages=true",
            &format!("-dMonoImageResolution={mono}"),
            "-dMonoImageDownsampleType=/Subsample",
            // Ghostscript 預設 1.5：來源解析度沒超過目標的 1.5 倍就整個跳過降採樣。
            // 實測結果是掃描件與型錄會出現「壓縮後大小完全沒變」。設 1.0 才符合直覺。
            "-dColorImageDownsampleThreshold=1.0",
            "-dGrayImageDownsampleThreshold=1.0",
            "-dMonoImageDownsampleThreshold=1.0",
            // 預設 true：沒被降採樣的 JPEG 會原樣複製，導致 -dJPEGQ 完全失效。
            "-dPassThroughJPEGImages=false",
            "-dPassThroughJPXImages=false",
            "-dAutoFilterColorImages=false",
            "-dColorImageFilter=/DCTEncode",
            "-dAutoFilterGrayImages=false",
            "-dGrayImageFilter=/DCTEncode",
            &format!("-dJPEGQ={q}"),
            "-dConvertCMYKImagesToRGB=true",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect()
    }
}

/// 每次執行都會帶上的參數。
fn base_args() -> Vec<&'static str> {
    vec![
        // 限制 PostScript 存取檔案系統。處理來路不明的 PDF 時這是必要防線，不可拿掉。
        "-dSAFER",
        "-dBATCH",
        "-dNOPAUSE",
        "-dQUIET",
        "-sDEVICE=pdfwrite",
        "-dCompatibilityLevel=1.7",
        "-dSubsetFonts=true",
        "-dCompressFonts=true",
        "-dDetectDuplicateImages=true",
        // Ghostscript 預設會依內文方向自動轉頁，對使用者來說是莫名其妙的副作用。
        "-dAutoRotatePages=/None",
        "-dPreserveAnnots=true",
    ]
}

/// 依序在幾個位置找 Ghostscript（Windows：`gswin64c.exe`；macOS：`gs`）。
///
/// `resource_dir` 是打包後隨 App 附帶的位置；開發時會落到 vendor/ 或系統安裝。
pub fn locate(resource_dir: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = crate::platform::find_bundled(resource_dir, crate::platform::bin::GHOSTSCRIPT, false) {
        return Some(p);
    }
    locate_system()
}

/// 最後才找系統安裝的版本（開發用；正式版一律使用內嵌的，避免版本行為飄移）
#[cfg(windows)]
fn locate_system() -> Option<PathBuf> {
    for base in [r"C:\Program Files\gs", r"C:\Program Files (x86)\gs"] {
        let Ok(dir) = std::fs::read_dir(base) else { continue };
        let mut versions: Vec<PathBuf> = dir
            .flatten()
            .map(|e| e.path().join("bin/gswin64c.exe"))
            .filter(|p| p.exists())
            .collect();
        versions.sort();
        if let Some(p) = versions.pop() {
            return Some(p);
        }
    }
    None
}

#[cfg(not(windows))]
fn locate_system() -> Option<PathBuf> {
    crate::platform::find_on_system("gs")
}

/// 回傳 Ghostscript 版本字串，用來在啟動時確認引擎可用。
pub fn version(gs: &Path) -> Result<String, String> {
    let out = build_cmd(gs)
        .arg("--version")
        .output()
        .map_err(|e| format!("無法執行壓縮引擎：{e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 壓縮單一檔案。input 與 output 必須是不同路徑。
pub fn compress(gs: &Path, input: &Path, output: &Path, level: Level) -> Result<(), String> {
    let out = build_cmd(gs)
        .args(base_args())
        .args(level.args())
        .arg(format!("-sOutputFile={}", output.display()))
        .arg(input)
        .output()
        .map_err(|e| format!("無法執行壓縮引擎：{e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.lines().last().unwrap_or("未知錯誤").trim();
        return Err(format!("壓縮失敗：{msg}"));
    }
    // Ghostscript 對某些損毀檔會回傳 0 但產不出東西
    match std::fs::metadata(output) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => Err("壓縮失敗：未產生輸出，檔案可能已損毀或受密碼保護".into()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageFormat {
    Jpeg,
    Png,
}

impl ImageFormat {
    /// JPEG 適合照片與掃描件，PNG 無損、適合圖表與文字頁。
    fn device_args(self) -> [&'static str; 2] {
        match self {
            ImageFormat::Jpeg => ["-sDEVICE=jpeg", "-dJPEGQ=90"],
            // png16m 是 24-bit 全彩；pngalpha 會帶透明背景，對文件掃描反而礙事。
            ImageFormat::Png => ["-sDEVICE=png16m", "-dDownScaleFactor=1"],
        }
    }

    pub fn extension(self) -> &'static str {
        match self {
            ImageFormat::Jpeg => "jpg",
            ImageFormat::Png => "png",
        }
    }
}

/// 把單一頁面算成圖片。
///
/// 刻意一頁一次呼叫，而不是用 Ghostscript 的 `%03d` 樣板一次輸出整份。
/// 樣板會把檔案寫到我們管不到的名字上，等於在暫存管理之外多開一條路徑；
/// 一頁一檔則能讓每個位元組都落在受管理的暫存檔裡。代價是每頁多一次行程啟動。
pub fn render_page(
    gs: &Path,
    input: &Path,
    page: u32,
    dpi: u32,
    format: ImageFormat,
    out: &Path,
) -> Result<(), String> {
    if !(20..=600).contains(&dpi) {
        return Err("解析度請設在 20 到 600 dpi 之間".into());
    }
    let result = build_cmd(gs)
        .args(["-dSAFER", "-dBATCH", "-dNOPAUSE", "-dQUIET"])
        .args(format.device_args())
        // 依 CropBox 出圖，否則裁切過的頁面會把已裁掉的部分又畫回來。
        .arg("-dUseCropBox")
        .arg(format!("-r{dpi}"))
        .arg(format!("-dFirstPage={page}"))
        .arg(format!("-dLastPage={page}"))
        .arg(format!("-sOutputFile={}", out.display()))
        .arg(input)
        .output()
        .map_err(|e| format!("無法執行轉檔引擎：{e}"))?;

    if !result.status.success() {
        let err = String::from_utf8_lossy(&result.stderr);
        return Err(format!(
            "第 {page} 頁轉檔失敗：{}",
            err.lines().last().unwrap_or("未知錯誤").trim()
        ));
    }
    match std::fs::metadata(out) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => Err(format!("第 {page} 頁未產生輸出")),
    }
}

/// 抽出整份文件的文字。
///
/// 走 `txtwrite` 裝置是因為它會處理字型編碼與來源字型的 ToUnicode CMap——
/// 自己剖析內容串流的話，中文 PDF 幾乎一定會拿到亂碼。
///
/// 輸出導到 stdout 而不是暫存檔：文字內容一個位元組都不落地。
pub fn extract_text(gs: &Path, input: &Path) -> Result<String, String> {
    let out = build_cmd(gs)
        .args([
            "-dSAFER",
            "-dBATCH",
            "-dNOPAUSE",
            "-dQUIET",
            "-sDEVICE=txtwrite",
            "-sOutputFile=-",
        ])
        .arg(input)
        .output()
        .map_err(|e| format!("無法執行文字抽取：{e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(format!(
            "文字抽取失敗：{}",
            err.lines().last().unwrap_or("未知錯誤").trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 重建損毀的 PDF。
///
/// Ghostscript 的剖析器容錯性很高——交叉參照表壞掉、物件缺尾、串流長度錯誤
/// 這些它多半能自行復原，然後重新寫出一份結構正確的檔案。
///
/// 這裡刻意關掉所有降採樣並保留原始 JPEG：使用者要的是「把檔案救回來」，
/// 不是順便被壓縮。修復後想縮小，再跑一次壓縮工具即可。
pub fn repair(gs: &Path, input: &Path, output: &Path) -> Result<(), String> {
    let result = build_cmd(gs)
        .args(base_args())
        .args([
            "-dPDFSTOPONERROR=false",
            "-dPDFSTOPONWARNING=false",
            "-dDownsampleColorImages=false",
            "-dDownsampleGrayImages=false",
            "-dDownsampleMonoImages=false",
            "-dAutoFilterColorImages=false",
            "-dPassThroughJPEGImages=true",
        ])
        .arg(format!("-sOutputFile={}", output.display()))
        .arg(input)
        .output()
        .map_err(|e| format!("無法執行修復引擎：{e}"))?;

    // 損毀檔幾乎一定會噴警告，所以不能把 stderr 有內容當成失敗；
    // 唯一可靠的判準是「有沒有生出一份讀得開的 PDF」。
    match std::fs::metadata(output) {
        Ok(m) if m.len() > 0 => Ok(()),
        _ => {
            let err = String::from_utf8_lossy(&result.stderr);
            Err(format!(
                "無法修復：{}",
                err.lines().last().unwrap_or("檔案損毀程度超出可復原範圍").trim()
            ))
        }
    }
}

fn build_cmd(gs: &Path) -> Command {
    let mut cmd = Command::new(gs);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 端到端：找得到引擎、壓得動真實檔案，而且**每一個等級都真的讓檔案變小**。
    ///
    /// 最後這個斷言是重點。Ghostscript 的 DownsampleThreshold 與 PassThroughJPEGImages
    /// 預設值曾讓「低度壓縮」對掃描件輸出 100% 原始大小——參數看起來完全正常，
    /// 結果卻是壓縮功能靜默失效。這個測試就是為了讓那種回歸無法通過 CI。
    ///
    /// 語料由 `python bench/make_corpus.py` 產生；不存在時跳過而非失敗。
    #[test]
    fn every_level_actually_shrinks_files() {
        let Some(gs) = locate(None) else {
            eprintln!("跳過：找不到 Ghostscript，見 vendor/README.md");
            return;
        };
        let corpus_dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("bench/corpus");

        // 掃描件與圖文混排是壓縮真正該生效的兩種檔案；純文字沒有影像可降採樣，不列入。
        for name in ["c2_scan.pdf", "c3_mixed.pdf"] {
            let src = corpus_dir.join(name);
            if !src.exists() {
                eprintln!("跳過 {name}：語料不存在，請先跑 bench/make_corpus.py");
                continue;
            }
            let orig = std::fs::metadata(&src).unwrap().len();

            for level in [Level::Extreme, Level::Recommended, Level::Low] {
                let out = std::env::temp_dir().join(format!("localpdf-it-{level:?}-{name}"));
                compress(&gs, &src, &out, level).expect("壓縮應該成功");
                let got = std::fs::metadata(&out).unwrap().len();
                std::fs::remove_file(&out).ok();

                assert!(
                    got < orig,
                    "{name} 在 {level:?} 等級下沒有變小（{got} vs {orig}）\
                     ——檢查 DownsampleThreshold 與 PassThroughJPEGImages"
                );
            }
        }
    }
}
