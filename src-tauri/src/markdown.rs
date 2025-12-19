//! PDF 轉 Markdown。
//!
//! 文字用 Ghostscript 的 `txtwrite` 裝置抽出來——它會處理字型編碼與 ToUnicode，
//! 中文實測正確。但**它不輸出字級**，所以標題層級無法從字型大小判斷，只能靠版面推測。
//!
//! 實測（見 `docs/pdf-to-office.md`）：txtwrite 的輸出沒有空行、沒有換頁符、
//! 每行縮排都一樣。唯一可用的結構訊號是**行寬**——文書處理器換行時會把行填滿，
//! 所以「滿寬的行」代表段落還沒結束，「明顯短的行」代表段落到此為止。

use std::path::Path;

type Result<T> = std::result::Result<T, String>;

/// 一行要多滿才算「被自動換行截斷」。低於這個比例視為段落結尾。
const FULL_LINE: f64 = 0.85;
/// 短到這個比例以下、又沒有句尾標點的獨立行，視為標題。
const HEADING_MAX: f64 = 0.6;

pub fn to_markdown(gs: &Path, input: &Path, output: &Path) -> Result<()> {
    let text = crate::gs::extract_text(gs, input)?;
    let md = render(&text);
    if md.trim().is_empty() {
        return Err("這份 PDF 沒有可抽取的文字，可能是掃描件——請先做 OCR".into());
    }
    std::fs::write(output, md).map_err(|e| format!("無法寫出檔案：{e}"))
}

/// 帶字級與區塊資訊的一行文字，由前端的 MuPDF 抽取而來。
#[derive(Debug, Clone, serde::Deserialize)]
pub struct TextLine {
    pub text: String,
    pub size: f64,
    #[allow(dead_code)]
    pub x: f64,
    #[allow(dead_code)]
    pub y: f64,
    /// 全文件唯一的區塊編號，大致等於段落。
    pub block: usize,
}

/// 字級要比內文大多少才算標題。1.15 是留給行距抖動的餘裕。
const HEADING_RATIO: f64 = 1.15;

/// 用字級與區塊資訊產生 Markdown。
///
/// 這條路徑比 [`render`] 準確得多——區塊邊界直接就是段落，標題層級來自實際字級
/// 而不是行寬猜測。代價是需要 MuPDF（見 `src/mupdf-text.ts` 的說明）。
pub fn render_structured(lines: &[TextLine]) -> String {
    if lines.is_empty() {
        return String::new();
    }

    // 內文字級 = 出現行數最多的那個字級。標題再多也不會比內文多。
    let body = dominant_size(lines);

    // 比內文大的字級由大到小排序，名次就是標題層級。
    let mut heading_sizes: Vec<f64> = lines
        .iter()
        .map(|l| l.size)
        .filter(|s| *s > body * HEADING_RATIO)
        .collect();
    heading_sizes.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    heading_sizes.dedup_by(|a, b| (*a - *b).abs() < 0.5);

    let mut out = String::new();
    for group in group_blocks(lines) {
        let text = join_lines(&group);
        if text.is_empty() {
            continue;
        }
        let size = group.iter().map(|l| l.size).fold(0.0, f64::max);

        if let Some(rank) = heading_sizes.iter().position(|s| (*s - size).abs() < 0.5) {
            let level = (rank + 1).min(6);
            out.push_str(&format!("\n{} {}\n", "#".repeat(level), text));
        } else if let Some(rest) = list_marker(&text) {
            out.push_str(&format!("- {}\n", escape(rest)));
        } else {
            out.push_str(&format!("\n{}\n", escape(&text)));
        }
    }
    out.trim().replace("\n\n\n", "\n\n") + "\n"
}

/// 出現行數最多的字級。
fn dominant_size(lines: &[TextLine]) -> f64 {
    let mut tally: Vec<(f64, usize)> = Vec::new();
    for line in lines {
        match tally.iter_mut().find(|(s, _)| (*s - line.size).abs() < 0.5) {
            Some((_, n)) => *n += 1,
            None => tally.push((line.size, 1)),
        }
    }
    tally
        .into_iter()
        .max_by_key(|(_, n)| *n)
        .map(|(s, _)| s)
        .unwrap_or(12.0)
}

/// 依區塊編號把行分組，保持原順序。
fn group_blocks(lines: &[TextLine]) -> Vec<Vec<TextLine>> {
    let mut out: Vec<Vec<TextLine>> = Vec::new();
    for line in lines {
        match out.last_mut() {
            Some(g) if g[0].block == line.block => g.push(line.clone()),
            _ => out.push(vec![line.clone()]),
        }
    }
    out
}

fn join_lines(group: &[TextLine]) -> String {
    let mut text = String::new();
    for line in group {
        let piece = line.text.trim();
        if piece.is_empty() {
            continue;
        }
        text = if text.is_empty() { piece.to_string() } else { join(&text, piece) };
    }
    text
}

/// 把抽出來的純文字整理成 Markdown。純函式，方便單獨測試。
///
/// 這是沒有 MuPDF 時的退路，只有行寬可以參考，準確度較低。
pub fn render(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.trim().is_empty())
        .collect();
    if lines.is_empty() {
        return String::new();
    }

    // 用第 90 百分位當「滿寬」的基準，避免被個別超長行拉走。
    let mut widths: Vec<usize> = lines.iter().map(|l| l.trim().chars().count()).collect();
    widths.sort_unstable();
    let full = widths[widths.len() * 9 / 10].max(1) as f64;

    let mut out = String::new();
    let mut seen_heading = false;
    let mut i = 0;

    while i < lines.len() {
        let current = lines[i].trim();

        if let Some(rest) = list_marker(current) {
            let (body, next) = gather(&lines, i, full, Some(rest));
            out.push_str(&format!("- {}\n", escape(&body)));
            i = next;
            continue;
        }

        let (body, next) = gather(&lines, i, full, None);
        let single = next == i + 1;
        let width = body.chars().count() as f64;

        if single && width <= full * HEADING_MAX && !ends_sentence(&body) {
            // 第一個標題當作文件標題，其餘一律 h2——沒有字級就無法判斷真正的層級。
            out.push_str(if seen_heading { "\n## " } else { "# " });
            out.push_str(&body);
            out.push('\n');
            seen_heading = true;
        } else {
            out.push_str(&format!("\n{}\n", escape(&body)));
        }
        i = next;
    }
    // 收尾：把多餘空行壓成一個
    out.trim().replace("\n\n\n", "\n\n") + "\n"
}

/// 從第 `start` 行開始，把被自動換行截斷的後續行接起來。
///
/// 回傳合併後的文字與下一個未處理的行號。
fn gather(lines: &[&str], start: usize, full: f64, strip: Option<&str>) -> (String, usize) {
    let mut body = strip.unwrap_or_else(|| lines[start].trim()).to_string();
    let mut i = start;
    while (lines[i].trim().chars().count() as f64) >= full * FULL_LINE && i + 1 < lines.len() {
        let next = lines[i + 1].trim();
        // 下一行自己是清單項目就不能併進來，那是新的區塊。
        if list_marker(next).is_some() {
            break;
        }
        body = join(&body, next);
        i += 1;
    }
    (body, i + 1)
}

/// 接兩段文字。中文之間不加空白，英文單字之間要加。
fn join(left: &str, right: &str) -> String {
    let a = left.chars().last();
    let b = right.chars().next();
    let needs_space = matches!((a, b), (Some(x), Some(y)) if x.is_ascii_alphanumeric() && y.is_ascii_alphanumeric());
    if needs_space {
        format!("{left} {right}")
    } else {
        format!("{left}{right}")
    }
}

/// 認得常見的清單符號，回傳去掉符號後的內容。
fn list_marker(line: &str) -> Option<&str> {
    for prefix in ["- ", "* ", "• ", "‧ ", "· ", "◦ ", "‒ ", "– "] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some(rest.trim_start());
        }
    }
    // 「1. 」「(1) 」「3、」「一、」這類編號
    let body = line
        .strip_prefix('(')
        .or_else(|| line.strip_prefix('（'))
        .unwrap_or(line);

    let mut count = 0;
    for (idx, ch) in body.char_indices() {
        if ch.is_ascii_digit() || "一二三四五六七八九十".contains(ch) {
            count += 1;
            continue;
        }
        // 沒有數字，或數字後面不是編號用的標點，就不是清單
        if count == 0 || !matches!(ch, '.' | '、' | ')' | '）' | '：' | ':') {
            break;
        }
        let rest = body[idx + ch.len_utf8()..].trim_start();
        return (!rest.is_empty()).then_some(rest);
    }
    None
}

/// 結尾是句尾標點就不可能是標題。
fn ends_sentence(s: &str) -> bool {
    matches!(
        s.chars().last(),
        Some('。' | '，' | '、' | '；' | '：' | '.' | ',' | ';' | ':' | '!' | '?' | '！' | '？')
    )
}

/// 只跳脫會在行首造成非預期結構的字元。全面跳脫會讓輸出很難讀。
fn escape(s: &str) -> String {
    let first = s.chars().next();
    match first {
        Some(c @ ('#' | '>' | '-' | '*' | '+' | '|')) => format!("\\{c}{}", &s[c.len_utf8()..]),
        _ => s.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 取自實際 txtwrite 輸出的樣態：沒有空行、縮排一致、靠行寬區分段落。
    const SAMPLE: &str = "       專案進度報告
       第一章　背景說明
       本專案自去年啟動，目標是建立一套完全在本機執行的文件處理工具。過去一年間，團隊完成了核心架構的驗
       證，並確認了幾項關鍵技術的可行性。
       主要考量有三點：
       - 隱私：檔案不得離開使用者的裝置
       - 效能：大型掃描件必須能在數秒內處理完成
       第二章　目前進度
       壓縮功能已完成並通過實測。針對三百 dpi 的掃描件，建議設定可壓到原始大小的百分之十一，而畫質指標
       仍維持在零點九四以上。";

    #[test]
    fn rebuilds_paragraphs_from_hard_wraps() {
        let md = render(SAMPLE);
        assert!(
            md.contains("核心架構的驗證，並確認了幾項關鍵技術的可行性。"),
            "被自動換行截斷的段落應該接回去：\n{md}"
        );
        assert!(
            md.contains("百分之十一，而畫質指標仍維持在零點九四以上。"),
            "第二段也應該接回去：\n{md}"
        );
    }

    #[test]
    fn detects_headings() {
        let md = render(SAMPLE);
        assert!(md.starts_with("# 專案進度報告"), "第一個標題應為文件標題：\n{md}");
        assert!(md.contains("## 第一章　背景說明"), "章節應為 h2：\n{md}");
        assert!(md.contains("## 第二章　目前進度"), "第二章也是：\n{md}");
    }

    #[test]
    fn detects_list_items() {
        let md = render(SAMPLE);
        assert!(md.contains("- 隱私：檔案不得離開使用者的裝置"));
        assert!(md.contains("- 效能：大型掃描件必須能在數秒內處理完成"));
    }

    /// 結尾有句號的短行是段落，不是標題——這是最容易誤判的情況。
    #[test]
    fn short_line_with_period_is_not_a_heading() {
        let md = render("這是一個很長的段落內容用來建立行寬基準值需要夠長才行喔喔喔喔喔喔喔\n結束了。");
        assert!(!md.contains("# 結束了。"), "有句號就不該當標題：\n{md}");
    }

    #[test]
    fn numbered_lists_are_recognised() {
        assert_eq!(list_marker("1. 第一項"), Some("第一項"));
        assert_eq!(list_marker("(2) 第二項"), Some("第二項"));
        assert_eq!(list_marker("3、第三項"), Some("第三項"));
        assert_eq!(list_marker("一、緣起"), Some("緣起"), "中文數字編號很常見");
        assert_eq!(list_marker("2026 年的營收"), None, "年份不是清單編號");
        assert_eq!(list_marker("台北市政府公告"), None, "一般文字不該被當成清單");
    }

    #[test]
    fn english_words_keep_their_space() {
        assert_eq!(join("The quick brown", "fox jumps"), "The quick brown fox jumps");
        assert_eq!(join("中文結尾", "中文開頭"), "中文結尾中文開頭");
        assert_eq!(join("結尾。", "The quick"), "結尾。The quick");
    }

    #[test]
    fn leading_markdown_characters_are_escaped() {
        assert_eq!(escape("# 不是標題"), "\\# 不是標題");
        assert_eq!(escape("正常文字"), "正常文字");
    }

    #[test]
    fn empty_input_yields_empty_output() {
        assert_eq!(render(""), "");
        assert_eq!(render("   \n  \n"), "");
    }

    // ---- 帶字級的路徑 ----

    fn line(text: &str, size: f64, block: usize) -> TextLine {
        TextLine { text: text.into(), size, x: 60.0, y: 0.0, block }
    }

    fn doc() -> Vec<TextLine> {
        vec![
            line("專案進度報告", 24.0, 1),
            line("第一章　背景說明", 16.0, 2),
            line("本專案自去年啟動，目標是建立一套完全在本機執行的文件處理工具。", 10.5, 3),
            line("過去一年間，團隊完成了核心架構的驗證。", 10.5, 3),
            line("1. 隱私：檔案不得離開使用者的裝置", 10.5, 4),
            line("第二章　目前進度", 16.0, 5),
            line("壓縮功能已完成並通過實測。", 10.5, 6),
        ]
    }

    /// 字級直接給出層級，不必再靠行寬猜。
    #[test]
    fn heading_levels_come_from_font_size() {
        let md = render_structured(&doc());
        assert!(md.contains("# 專案進度報告"), "最大字級應為 h1：\n{md}");
        assert!(md.contains("## 第一章　背景說明"), "次大字級應為 h2：\n{md}");
        assert!(md.contains("## 第二章　目前進度"), "同字級應為同層級：\n{md}");
        assert!(!md.contains("# 壓縮功能"), "內文不該變標題：\n{md}");
    }

    /// 區塊編號就是段落邊界，同一區塊的行要接起來。
    #[test]
    fn lines_in_one_block_become_one_paragraph() {
        let md = render_structured(&doc());
        assert!(
            md.contains("文件處理工具。過去一年間"),
            "同區塊的兩行應該接成一段：\n{md}"
        );
    }

    #[test]
    fn structured_path_still_detects_lists() {
        let md = render_structured(&doc());
        assert!(md.contains("- 隱私：檔案不得離開使用者的裝置"), "\n{md}");
    }

    /// 整份文件字級一致時不該冒出任何標題。
    #[test]
    fn uniform_font_size_produces_no_headings() {
        let flat: Vec<TextLine> = (1..=5)
            .map(|i| line(&format!("第 {i} 段內容"), 12.0, i))
            .collect();
        let md = render_structured(&flat);
        assert!(!md.contains('#'), "沒有字級差異就不該有標題：\n{md}");
    }

    #[test]
    fn structured_empty_input() {
        assert_eq!(render_structured(&[]), "");
    }
}
