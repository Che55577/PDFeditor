//! 修正 pdf2docx 產出的 docx 裡的字型名稱。
//!
//! ## 問題
//!
//! pdf2docx 把 PDF 的字型名稱直接寫進 docx，但它做的正規化會產生**系統上不存在的名稱**。
//! 實測：來源 PDF 用的是 `AAAAAA+DFKaiShu-SB-Estd-BF`（標楷體的子集），pdf2docx 寫出
//! `w:ascii="DFKai" w:eastAsia="DFKai"`——Windows 上沒有叫 `DFKai` 的字型，
//! Word 與 LibreOffice 只好退回預設字型。
//!
//! 結果是中文變成另一套字（看起來像變粗了，其實是替代字型本身比較重），
//! 而且字寬一變，整段就得重新換行——也就是使用者說的「跑版」。
//!
//! ## 作法
//!
//! 列舉系統實際安裝的字型家族名稱，把 docx 裡對不上的名稱換成最接近的一個。
//! 完全對不上時退回呼叫端指定的字型。

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, String>;

/// 一個已安裝的字型家族。
///
/// 同一個字型檔常有多個名稱：拉丁名（`DFKai-SB`）與在地化名稱（`標楷體`）。
/// 這個區分很重要——實測把 `w:eastAsia` 設成中文名稱的版面比設成拉丁名準確得多，
/// 因為 `w:eastAsia` 本來就是給 CJK 字型用的欄位。
#[derive(Debug, Clone)]
pub struct Family {
    /// 全部別名，小寫，用來比對
    aliases: Vec<String>,
    /// 給 `w:ascii` / `w:hAnsi` / `w:cs`
    latin: String,
    /// 給 `w:eastAsia`；沒有中文名稱時退回 latin
    cjk: Option<String>,
}

impl Family {
    fn for_attr(&self, attr: &str) -> &str {
        match attr {
            "w:eastAsia" => self.cjk.as_deref().unwrap_or(&self.latin),
            _ => &self.latin,
        }
    }
}

fn has_cjk(s: &str) -> bool {
    s.chars().any(|c| ('\u{3000}'..='\u{9fff}').contains(&c))
}

/// 掃描系統字型目錄，一個字型檔收成一個 [`Family`]。
pub fn installed_families() -> Vec<Family> {
    let dir = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| r"C:\Windows".into()))
        .join("Fonts");
    let Ok(entries) = std::fs::read_dir(&dir) else { return Vec::new() };
    let mut out: Vec<Family> = Vec::new();

    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str()).map(str::to_lowercase);
        if !matches!(ext.as_deref(), Some("ttf" | "otf" | "ttc")) {
            continue;
        }
        let Ok(data) = std::fs::read(&path) else { continue };
        let faces = ttf_parser::fonts_in_collection(&data).unwrap_or(1);

        for index in 0..faces {
            let Ok(face) = ttf_parser::Face::parse(&data, index) else { continue };
            let mut aliases: Vec<String> = Vec::new();
            let mut latin: Option<String> = None;
            let mut cjk: Option<String> = None;

            for name in face.names() {
                // 1 = Family，16 = Typographic Family
                if !matches!(name.name_id, 1 | 16) {
                    continue;
                }
                let Some(text) = name.to_string() else { continue };
                let text = text.trim().to_string();
                if text.is_empty() {
                    continue;
                }
                let key = text.to_lowercase();
                if !aliases.contains(&key) {
                    aliases.push(key);
                }
                if has_cjk(&text) {
                    cjk.get_or_insert(text);
                } else {
                    latin.get_or_insert(text);
                }
            }

            if let Some(latin) = latin.or_else(|| cjk.clone()) {
                if !aliases.is_empty() {
                    out.push(Family { aliases, latin, cjk });
                }
            }
        }
    }
    out
}

/// 名稱正規化：去掉 PDF 子集前綴、空白與連字號，再轉小寫。
fn squash(name: &str) -> String {
    let lower = name.trim().to_lowercase();
    let bare = lower.rsplit_once('+').map(|(_, r)| r).unwrap_or(&lower);
    bare.chars().filter(|c| !c.is_whitespace() && *c != '-').collect()
}

/// 幫一個字型名稱找出最合適的已安裝家族。
///
/// 依序試：完全相同 → 正規化後相同 → 其中一方是另一方的前綴。
/// 前綴比對是關鍵，pdf2docx 寫出的 `DFKai` 要靠它才對得上 `DFKai-SB`。
fn resolve<'a>(name: &str, installed: &'a [Family]) -> Option<&'a Family> {
    let key = name.trim().to_lowercase();
    if let Some(hit) = installed.iter().find(|f| f.aliases.contains(&key)) {
        return Some(hit);
    }
    let target = squash(name);
    if target.is_empty() {
        return None;
    }

    let mut best: Option<(usize, &Family)> = None;
    for family in installed {
        for alias in &family.aliases {
            let other = squash(alias);
            if other == target {
                return Some(family);
            }
            if other.starts_with(&target) || target.starts_with(&other) {
                // 名稱越短代表越接近本體，不是某個粗體／斜體變體
                if best.map(|(s, _)| other.len() < s).unwrap_or(true) {
                    best = Some((other.len(), family));
                }
            }
        }
    }
    best.map(|(_, f)| f)
}

/// 把 docx 裡對不上的字型名稱換掉。回傳被替換的「原名 → 新名」對照，方便回報。
///
/// `fallback` 是完全找不到對應時要用的字型。
pub fn remap(docx: &Path, fallback: &str) -> Result<BTreeMap<String, String>> {
    const ENTRY: &str = "word/document.xml";
    const ATTRS: [&str; 4] = ["w:ascii", "w:hAnsi", "w:eastAsia", "w:cs"];

    let installed = installed_families();
    let file = std::fs::File::open(docx).map_err(|e| format!("無法開啟輸出檔：{e}"))?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| format!("輸出檔不是有效的 docx：{e}"))?;

    // 先讀出所有內容，才能原地覆寫
    let mut entries: Vec<(String, Vec<u8>)> = Vec::with_capacity(archive.len());
    for i in 0..archive.len() {
        let mut item = archive.by_index(i).map_err(|e| format!("讀取 docx 失敗：{e}"))?;
        let name = item.name().to_string();
        let mut buf = Vec::new();
        item.read_to_end(&mut buf).map_err(|e| format!("讀取 docx 失敗：{e}"))?;
        entries.push((name, buf));
    }
    drop(archive);

    let mut changes: BTreeMap<String, String> = BTreeMap::new();
    for (name, data) in entries.iter_mut() {
        if name != ENTRY {
            continue;
        }
        let mut text = String::from_utf8_lossy(data).into_owned();

        // 收集目前用到的所有字型名稱
        let mut used: Vec<String> = Vec::new();
        for attr in ATTRS {
            let needle = format!("{attr}=\"");
            let mut rest = text.as_str();
            while let Some(at) = rest.find(&needle) {
                rest = &rest[at + needle.len()..];
                if let Some(end) = rest.find('"') {
                    let value = rest[..end].to_string();
                    if !value.is_empty() && !used.contains(&value) {
                        used.push(value);
                    }
                    rest = &rest[end..];
                }
            }
        }

        for original in used {
            let found = resolve(&original, &installed);
            for attr in ATTRS {
                let replacement = match found {
                    // w:eastAsia 要放中文名稱，其餘放拉丁名——實測這樣的版面準確得多
                    Some(family) => family.for_attr(attr).to_string(),
                    None => fallback.to_string(),
                };
                if replacement == original {
                    continue;
                }
                text = text.replace(
                    &format!("{attr}=\"{original}\""),
                    &format!("{attr}=\"{replacement}\""),
                );
                changes.insert(format!("{original} ({attr})"), replacement);
            }
        }
        *data = text.into_bytes();
    }

    if changes.is_empty() {
        return Ok(changes);
    }

    let out = std::fs::File::create(docx).map_err(|e| format!("無法寫回輸出檔：{e}"))?;
    let mut writer = zip::ZipWriter::new(out);
    let options: zip::write::FileOptions<'_, ()> =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    for (name, data) in entries {
        writer.start_file(name, options).map_err(|e| format!("寫入 docx 失敗：{e}"))?;
        writer.write_all(&data).map_err(|e| format!("寫入 docx 失敗：{e}"))?;
    }
    writer.finish().map_err(|e| format!("寫入 docx 失敗：{e}"))?;
    Ok(changes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_common_windows_families() {
        let installed = installed_families();
        if installed.is_empty() {
            eprintln!("跳過：讀不到系統字型");
            return;
        }
        assert!(
            resolve("Arial", &installed).is_some(),
            "應該找得到 Arial"
        );
    }

    /// 這是整個模組存在的理由：pdf2docx 寫出的 DFKai 要能對上實際安裝的標楷體。
    #[test]
    fn resolves_truncated_name_to_installed_family() {
        let installed = installed_families();
        let Some(found) = resolve("DFKai", &installed) else {
            eprintln!("跳過：系統沒有標楷體");
            return;
        };
        assert!(found.latin.to_lowercase().starts_with("dfkai"), "實得 {}", found.latin);
        assert_ne!(found.latin, "DFKai", "必須換成實際安裝的名稱");
    }

    /// w:eastAsia 要放中文名稱——實測這樣的版面比放拉丁名準確得多。
    #[test]
    fn east_asia_prefers_the_localized_name() {
        let installed = installed_families();
        let Some(found) = resolve("DFKai", &installed) else { return };
        if found.cjk.is_none() {
            eprintln!("跳過：這份標楷體沒有中文名稱");
            return;
        }
        let east = found.for_attr("w:eastAsia");
        assert!(has_cjk(east), "w:eastAsia 應該用中文名稱，實得 {east}");
        assert!(!has_cjk(found.for_attr("w:ascii")), "w:ascii 應該用拉丁名");
    }

    #[test]
    fn strips_pdf_subset_prefix() {
        let installed = installed_families();
        if installed.is_empty() {
            return;
        }
        let found = resolve("ABCDEF+Arial", &installed).expect("去掉子集前綴後應該對得上");
        assert_eq!(found.latin, "Arial");
    }

    #[test]
    fn unknown_name_has_no_match() {
        let installed = installed_families();
        if installed.is_empty() {
            return;
        }
        assert!(resolve("ZZNotARealFontZZ", &installed).is_none());
    }
}
