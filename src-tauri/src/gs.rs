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

/// 依序在幾個位置找 gswin64c.exe。
///
/// `resource_dir` 是打包後隨 App 附帶的位置；開發時會落到 vendor/ 或系統安裝。
pub fn locate(resource_dir: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();

    if let Some(res) = resource_dir {
        candidates.push(res.join("ghostscript/bin/gswin64c.exe"));
    }
    // 開發時：專案根目錄的 vendor/
    if let Ok(exe) = std::env::current_exe() {
        for up in exe.ancestors().take(6) {
            candidates.push(up.join("vendor/ghostscript/bin/gswin64c.exe"));
        }
    }
    if let Some(p) = candidates.iter().find(|p| p.exists()) {
        return Some(p.clone());
    }

    // 最後才找系統安裝的版本（開發用；正式版一律使用內嵌的，避免版本行為飄移）
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
