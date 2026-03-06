//! 平台差異集中在這裡：外部執行檔的名稱與位置、系統字型目錄。
//!
//! 其他模組只問「去哪裡找」，不自己寫死 `C:\Windows` 或 `/Applications`。
//! Windows 的行為與移植前完全相同；macOS 是新增的分支。

use std::path::{Path, PathBuf};

/// 打包資源（或開發時 `vendor/`）底下的相對路徑。
pub mod bin {
    #[cfg(windows)]
    pub const GHOSTSCRIPT: &str = "ghostscript/bin/gswin64c.exe";
    #[cfg(not(windows))]
    pub const GHOSTSCRIPT: &str = "ghostscript/bin/gs";

    /// Windows 版是攤平的 exe + DLL；macOS 版保留官方的 bin/ + lib/ 結構，
    /// 因為 `qpdf` 以 `@executable_path/../lib` 找 `libqpdf.dylib`。
    #[cfg(windows)]
    pub const QPDF: &str = "qpdf/qpdf.exe";
    #[cfg(not(windows))]
    pub const QPDF: &str = "qpdf/bin/qpdf";

    /// python-build-standalone 的目錄結構：Windows 直接在根目錄，Unix 在 bin/。
    #[cfg(windows)]
    pub const PYTHON: &str = "pyext/python.exe";
    #[cfg(not(windows))]
    pub const PYTHON: &str = "pyext/bin/python3";
}

/// 依序在「打包資源 → 開發時的 vendor/ → 執行檔所在位置往上」找 `rel`。
///
/// `also_beside_exe` 給擴充包用：它安裝後會落在程式目錄底下，而不是資源目錄。
pub fn find_bundled(resource_dir: Option<&Path>, rel: &str, also_beside_exe: bool) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(res) = resource_dir {
        candidates.push(res.join(rel));
    }
    if let Ok(exe) = std::env::current_exe() {
        for up in exe.ancestors().take(6) {
            candidates.push(up.join("vendor").join(rel));
            if also_beside_exe {
                candidates.push(up.join(rel));
            }
        }
    }
    candidates.into_iter().find(|p| p.exists())
}

/// 系統上的工具（開發用的備援；正式版一律優先用內嵌的，避免版本行為飄移）。
#[cfg(not(windows))]
pub fn find_on_system(name: &str) -> Option<PathBuf> {
    // GUI App 不會繼承 shell 的 PATH，所以 Homebrew 的位置要明列。
    let mut dirs: Vec<PathBuf> = vec!["/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    if let Some(path) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path));
    }
    dirs.into_iter().map(|d| d.join(name)).find(|p| p.is_file())
}

/// 系統字型目錄。只掃這幾層，不遞迴。
pub fn font_dirs() -> Vec<PathBuf> {
    #[cfg(windows)]
    {
        vec![PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into())).join("Fonts")]
    }
    #[cfg(target_os = "macos")]
    {
        let mut dirs: Vec<PathBuf> = vec![
            "/System/Library/Fonts".into(),
            // Arial、宋體等多數「非系統介面」字型在這裡
            "/System/Library/Fonts/Supplemental".into(),
            "/Library/Fonts".into(),
        ];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join("Library/Fonts"));
        }
        dirs
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let mut dirs: Vec<PathBuf> = vec!["/usr/share/fonts".into(), "/usr/local/share/fonts".into()];
        if let Some(home) = std::env::var_os("HOME") {
            dirs.push(PathBuf::from(home).join(".local/share/fonts"));
        }
        dirs
    }
}

/// 浮水印、頁碼可選的字型。
struct Candidate {
    label: &'static str,
    file: &'static str,
    /// 字型集（.ttc）裡要挑的家族名稱。`None` 代表直接用第 0 個。
    family: Option<&'static str>,
}

/// 標楷體排第一：它是單一 TTF（好處理），而且是台灣公文書最常用的字體。
#[cfg(windows)]
const CANDIDATES: &[Candidate] = &[
    Candidate { label: "標楷體", file: "kaiu.ttf", family: None },
    Candidate { label: "微軟正黑體", file: "msjh.ttc", family: None },
    Candidate { label: "新細明體", file: "mingliu.ttc", family: None },
    Candidate { label: "Arial", file: "arial.ttf", family: None },
    Candidate { label: "Calibri", file: "calibri.ttf", family: None },
];

/// macOS 預設沒有標楷體；有些人會自行安裝 kaiu.ttf，所以仍然排第一。
/// 系統內建的中文字型都是 .ttc，裡面同時有簡繁與多種字重，要按家族名稱挑。
#[cfg(not(windows))]
const CANDIDATES: &[Candidate] = &[
    Candidate { label: "標楷體", file: "kaiu.ttf", family: None },
    Candidate { label: "蘋方-繁", file: "PingFang.ttc", family: Some("PingFang TC") },
    Candidate { label: "黑體-繁", file: "STHeiti Medium.ttc", family: Some("Heiti TC") },
    Candidate { label: "宋體-繁", file: "Songti.ttc", family: Some("Songti TC") },
    Candidate { label: "Arial", file: "Arial.ttf", family: None },
];

/// 一個可用的字型：顯示名稱、檔案路徑、字型集裡的索引。
pub struct FontFace {
    pub label: String,
    pub path: PathBuf,
    pub index: u32,
}

/// 系統上實際存在的候選字型，依偏好順序。
pub fn available_fonts() -> Vec<FontFace> {
    let dirs = font_dirs();
    CANDIDATES
        .iter()
        .filter_map(|c| {
            let path = dirs.iter().map(|d| d.join(c.file)).find(|p| p.exists())?;
            let index = match c.family {
                None => 0,
                // 對不到指定家族就不列——寧可少一個選項，也不要給錯字型（例如簡體）
                Some(family) => face_index(&path, family)?,
            };
            Some(FontFace { label: c.label.to_string(), path, index })
        })
        .collect()
}

/// 在字型集裡找家族名稱相符的那一個，偏好一般字重。
fn face_index(path: &Path, family: &str) -> Option<u32> {
    let data = std::fs::read(path).ok()?;
    let count = ttf_parser::fonts_in_collection(&data).unwrap_or(1);
    let mut fallback: Option<u32> = None;
    for index in 0..count {
        let Ok(face) = ttf_parser::Face::parse(&data, index) else { continue };
        let names: Vec<(u16, String)> = face
            .names()
            .into_iter()
            .filter_map(|n| Some((n.name_id, n.to_string()?)))
            .collect();
        let has = |ids: &[u16], want: &dyn Fn(&str) -> bool| {
            names.iter().any(|(id, text)| ids.contains(id) && want(text.trim()))
        };
        // 1 = Family、16 = Typographic Family；2 = Subfamily、17 = Typographic Subfamily
        if !has(&[1, 16], &|t| t.eq_ignore_ascii_case(family)) {
            continue;
        }
        if has(&[2, 17], &|t| matches!(t.to_lowercase().as_str(), "regular" | "medium" | "w3")) {
            return Some(index);
        }
        fallback.get_or_insert(index);
    }
    fallback
}

/// 測試用：第一個含中文的可用字型。
#[cfg(test)]
pub fn cjk_test_font() -> Option<FontFace> {
    available_fonts()
        .into_iter()
        .find(|f| f.label.chars().any(|c| ('\u{4e00}'..='\u{9fff}').contains(&c)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_dirs_are_absolute() {
        assert!(font_dirs().iter().all(|d| d.is_absolute()));
    }

    #[test]
    fn available_fonts_point_at_real_files() {
        for f in available_fonts() {
            assert!(f.path.is_file(), "{} 不存在", f.path.display());
            let data = std::fs::read(&f.path).unwrap();
            assert!(ttf_parser::Face::parse(&data, f.index).is_ok(), "{} 的索引 {} 無效", f.label, f.index);
        }
    }
}
