//! 在既有頁面上疊加文字：浮水印與頁碼。
//!
//! 兩者共用同一套機制——**只在原內容之上追加一段內容串流，原內容一個位元組都不動**。
//! 這也是 iLovePDF 的「Edit PDF」實際在做的事（見 `docs/edit-pdf-analysis.md`）。

use std::path::{Path, PathBuf};

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

use crate::font::Font;
use crate::pages;

type Result<T> = std::result::Result<T, String>;

#[derive(Clone, Debug, serde::Deserialize)]
pub struct TextStyle {
    pub font_path: PathBuf,
    #[serde(default)]
    pub font_index: u32,
    pub size: f64,
    /// RGB，各分量 0.0 ~ 1.0
    pub color: [f64; 3],
    /// 0.0 ~ 1.0
    pub opacity: f64,
}

impl TextStyle {
    fn validate(&self) -> Result<()> {
        if !(1.0..=400.0).contains(&self.size) {
            return Err("字級請設在 1 到 400 之間".into());
        }
        if !(0.0..=1.0).contains(&self.opacity) {
            return Err("透明度必須介於 0 與 1 之間".into());
        }
        if self.color.iter().any(|c| !(0.0..=1.0).contains(c)) {
            return Err("色彩值必須介於 0 與 1 之間".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Corner {
    TopLeft,
    TopCenter,
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

/// 斜向文字浮水印，置中於每一頁。
pub fn watermark(
    input: &Path,
    output: &Path,
    text: &str,
    style: &TextStyle,
    angle_deg: f64,
    targets: &[u32],
) -> Result<()> {
    if text.trim().is_empty() {
        return Err("浮水印文字不能空白".into());
    }
    style.validate()?;

    let mut doc = load(input)?;
    let mut font = Font::load(&style.font_path, style.font_index)?;
    let prepared = font.prepare(text)?;
    let font_id = font.embed(&mut doc)?;

    let (sin, cos) = angle_deg.to_radians().sin_cos();
    let width = prepared.width_em * style.size;

    for page_id in resolve(&doc, targets)? {
        let [x0, y0, x1, y1] = page_box(&doc, page_id)?;
        let (cx, cy) = ((x0 + x1) / 2.0, (y0 + y1) / 2.0);

        // 讓文字的中心落在頁面中心：先沿文字方向退半個寬度，
        // 再往垂直方向退約略半個字高（0.35 em 接近多數字型的視覺中線）。
        let half = width / 2.0;
        let rise = style.size * 0.35;
        let tx = cx - half * cos + rise * sin;
        let ty = cy - half * sin - rise * cos;

        let body = format!(
            "BT /Fwm {size} Tf {r:.4} {g:.4} {b:.4} rg \
             {cos:.6} {sin:.6} {nsin:.6} {cos:.6} {tx:.3} {ty:.3} Tm <{hex}> Tj ET",
            size = style.size,
            r = style.color[0],
            g = style.color[1],
            b = style.color[2],
            nsin = -sin,
            hex = prepared.hex,
        );
        stamp(&mut doc, page_id, font_id, style.opacity, &body)?;
    }
    save(&mut doc, output)
}

/// 在指定角落標上頁碼。
///
/// `template` 內的 `{n}` 會換成頁碼、`{total}` 換成總頁數，例如 `第 {n} 頁，共 {total} 頁`。
pub fn page_numbers(
    input: &Path,
    output: &Path,
    template: &str,
    style: &TextStyle,
    corner: Corner,
    start_at: i64,
    margin: f64,
    targets: &[u32],
) -> Result<()> {
    if template.trim().is_empty() {
        return Err("頁碼格式不能空白".into());
    }
    if margin < 0.0 {
        return Err("邊界不能是負值".into());
    }
    style.validate()?;

    let mut doc = load(input)?;
    let total = doc.get_pages().len();
    let chosen = resolve(&doc, targets)?;

    // 所有頁的文字都要先登記，字型子集才會涵蓋到每一個用得到的數字。
    let mut font = Font::load(&style.font_path, style.font_index)?;
    let mut labels = Vec::with_capacity(chosen.len());
    for (i, page_id) in chosen.iter().enumerate() {
        let text = template
            .replace("{n}", &(start_at + i as i64).to_string())
            .replace("{total}", &total.to_string());
        labels.push((*page_id, font.prepare(&text)?));
    }
    let font_id = font.embed(&mut doc)?;

    for (page_id, prepared) in labels {
        let [x0, y0, x1, y1] = page_box(&doc, page_id)?;
        let width = prepared.width_em * style.size;

        let x = match corner {
            Corner::TopLeft | Corner::BottomLeft => x0 + margin,
            Corner::TopCenter | Corner::BottomCenter => (x0 + x1 - width) / 2.0,
            Corner::TopRight | Corner::BottomRight => x1 - margin - width,
        };
        let y = match corner {
            // 上緣要再往下讓出一個字高，否則文字會頂在裁切線外。
            Corner::TopLeft | Corner::TopCenter | Corner::TopRight => y1 - margin - style.size,
            _ => y0 + margin,
        };

        let body = format!(
            "BT /Fwm {size} Tf {r:.4} {g:.4} {b:.4} rg 1 0 0 1 {x:.3} {y:.3} Tm <{hex}> Tj ET",
            size = style.size,
            r = style.color[0],
            g = style.color[1],
            b = style.color[2],
            hex = prepared.hex,
        );
        stamp(&mut doc, page_id, font_id, style.opacity, &body)?;
    }
    save(&mut doc, output)
}

// ---- 內部共用 ----

fn load(path: &Path) -> Result<Document> {
    Document::load(path).map_err(|e| format!("無法讀取 PDF：{e}"))
}

fn save(doc: &mut Document, output: &Path) -> Result<()> {
    doc.compress();
    doc.save(output).map(|_| ()).map_err(|e| format!("無法寫出 PDF：{e}"))
}

/// 把 1-based 頁碼換成物件編號；清單為空代表整份文件。
fn resolve(doc: &Document, targets: &[u32]) -> Result<Vec<ObjectId>> {
    let map = doc.get_pages();
    if targets.is_empty() {
        return Ok(map.into_values().collect());
    }
    targets
        .iter()
        .map(|n| {
            map.get(n)
                .copied()
                .ok_or_else(|| format!("第 {n} 頁不存在（本檔共 {} 頁）", map.len()))
        })
        .collect()
}

/// 疊加內容要以 CropBox 為準——使用者看到的是裁切後的範圍。
fn page_box(doc: &Document, page_id: ObjectId) -> Result<[f64; 4]> {
    pages::inherited_rect(doc, page_id, b"CropBox")
        .or_else(|| pages::inherited_rect(doc, page_id, b"MediaBox"))
        .ok_or_else(|| "頁面沒有可用的尺寸資訊".to_string())
}

/// 把一段繪圖指令疊到頁面最上層。
fn stamp(
    doc: &mut Document,
    page_id: ObjectId,
    font_id: ObjectId,
    opacity: f64,
    body: &str,
) -> Result<()> {
    let mut gs = Dictionary::new();
    gs.set("Type", Object::Name(b"ExtGState".to_vec()));
    gs.set("ca", Object::Real(opacity as f32)); // 填色透明度
    gs.set("CA", Object::Real(opacity as f32)); // 描邊透明度
    let gs_id = doc.add_object(gs);

    let mut resources = effective_resources(doc, page_id);
    merge_resource(doc, &mut resources, b"Font", "Fwm", font_id);
    merge_resource(doc, &mut resources, b"ExtGState", "GSwm", gs_id);

    // 原內容可能留下不平衡的繪圖狀態，所以在最前面補一個 q、我們自己的內容前補 Q，
    // 確保疊加層永遠從乾淨的狀態開始。
    let opening = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
    let overlay = doc.add_object(Stream::new(
        Dictionary::new(),
        format!("Q\nq\n/GSwm gs\n{body}\nQ\n").into_bytes(),
    ));

    let page = doc
        .get_object(page_id)
        .and_then(|o| o.as_dict())
        .map_err(|e| format!("頁面結構異常：{e}"))?;
    let mut streams: Vec<Object> = match page.get(b"Contents") {
        Ok(Object::Reference(id)) => vec![Object::Reference(*id)],
        Ok(Object::Array(items)) => items.clone(),
        _ => Vec::new(),
    };
    streams.insert(0, Object::Reference(opening));
    streams.push(Object::Reference(overlay));

    let Ok(Object::Dictionary(page)) = doc.get_object_mut(page_id) else {
        return Err("頁面結構異常".into());
    };
    page.set("Contents", Object::Array(streams));
    page.set("Resources", Object::Dictionary(resources));
    Ok(())
}

/// /Resources 是可繼承屬性，頁面自己沒有時要往上找，找到後複製一份掛回該頁。
fn effective_resources(doc: &Document, page_id: ObjectId) -> Dictionary {
    let mut id = page_id;
    for _ in 0..32 {
        let Ok(dict) = doc.get_object(id).and_then(|o| o.as_dict()) else { break };
        if let Ok(res) = dict.get(b"Resources") {
            if let Ok(d) = doc.dereference(res).map(|(_, o)| o).and_then(|o| o.as_dict()) {
                return d.clone();
            }
        }
        let Ok(parent) = dict.get(b"Parent").and_then(|o| o.as_reference()) else { break };
        id = parent;
    }
    Dictionary::new()
}

/// 把一筆資源加進某個類別，保留該類別原有的項目。
///
/// 不能直接覆寫整個 /Font——那會把頁面原本用的字型全部抹掉，內文就變成空白。
fn merge_resource(
    doc: &Document,
    resources: &mut Dictionary,
    category: &[u8],
    key: &str,
    value: ObjectId,
) {
    let mut group = resources
        .get(category)
        .ok()
        .and_then(|o| doc.dereference(o).ok())
        .and_then(|(_, o)| o.as_dict().ok())
        .cloned()
        .unwrap_or_default();
    group.set(key, Object::Reference(value));
    resources.set(
        String::from_utf8_lossy(category).to_string(),
        Object::Dictionary(group),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    const KAIU: &str = r"C:\Windows\Fonts\kaiu.ttf";

    fn style() -> TextStyle {
        TextStyle {
            font_path: PathBuf::from(KAIU),
            font_index: 0,
            size: 48.0,
            color: [0.5, 0.5, 0.5],
            opacity: 0.3,
        }
    }

    fn corpus() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("bench/corpus/c1_text.pdf");
        (p.exists() && Path::new(KAIU).exists()).then_some(p)
    }

    fn tmp(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("localpdf-annot-{tag}.pdf"))
    }

    #[test]
    fn watermark_keeps_page_count_and_grows_file() {
        let Some(src) = corpus() else {
            eprintln!("跳過：需要語料與標楷體");
            return;
        };
        let out = tmp("wm");
        watermark(&src, &out, "機密文件", &style(), 45.0, &[]).unwrap();

        let doc = Document::load(&out).unwrap();
        assert_eq!(doc.get_pages().len(), 100, "疊加不應改變頁數");
        // 內嵌了字型子集，檔案一定會變大；沒變大代表根本沒寫進去。
        assert!(
            std::fs::metadata(&out).unwrap().len() > std::fs::metadata(&src).unwrap().len(),
            "輸出應包含新增的字型與內容串流"
        );
    }

    /// 原內容必須原封不動——這是疊加式註記與「真編輯」的分界。
    #[test]
    fn watermark_preserves_original_content_streams() {
        let Some(src) = corpus() else { return };
        let out = tmp("wm-keep");
        watermark(&src, &out, "草稿", &style(), 45.0, &[1]).unwrap();

        let before = Document::load(&src).unwrap();
        let after = Document::load(&out).unwrap();
        let first = after.get_pages()[&1];
        let original = before.get_page_content(before.get_pages()[&1]);
        let combined = after.get_page_content(first);

        let orig_text = String::from_utf8_lossy(&original).to_string();
        let new_text = String::from_utf8_lossy(&combined).to_string();
        assert!(new_text.contains(orig_text.trim()), "原本的繪圖指令必須完整保留");
        assert!(new_text.contains("/GSwm gs"), "疊加層應該存在");
    }

    #[test]
    fn page_numbers_expand_placeholders() {
        let Some(src) = corpus() else { return };
        let out = tmp("pn");
        page_numbers(&src, &out, "第 {n} 頁，共 {total} 頁", &style(), Corner::BottomCenter, 1, 36.0, &[])
            .unwrap();
        assert_eq!(Document::load(&out).unwrap().get_pages().len(), 100);
    }

    #[test]
    fn rejects_bad_style_and_empty_text() {
        let Some(src) = corpus() else { return };
        let mut bad = style();
        bad.opacity = 5.0;
        assert!(watermark(&src, &tmp("x"), "字", &bad, 0.0, &[]).is_err());

        assert!(watermark(&src, &tmp("x"), "   ", &style(), 0.0, &[]).is_err());
        assert!(page_numbers(&src, &tmp("x"), "", &style(), Corner::TopLeft, 1, 36.0, &[]).is_err());
    }

    #[test]
    fn out_of_range_page_is_reported() {
        let Some(src) = corpus() else { return };
        let err = watermark(&src, &tmp("x"), "字", &style(), 0.0, &[9999]).unwrap_err();
        assert!(err.contains("不存在"), "應說明頁碼超出範圍：{err}");
    }
}
