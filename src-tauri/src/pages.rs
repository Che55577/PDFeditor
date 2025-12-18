//! 頁面層的結構操作：合併、選頁、旋轉。
//!
//! 這些刻意不走 Ghostscript。`-sDEVICE=pdfwrite` 會把整份文件重新算過一遍——
//! 慢，而且會丟掉書籤、連結與表單欄位。純粹搬動頁面時應該只動物件結構，
//! 內容串流原封不動。lopdf 是純 Rust、無 C 相依，之後移植 Android 也不會卡住。

use std::collections::BTreeMap;
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId};

type Result<T> = std::result::Result<T, String>;

fn load(path: &Path) -> Result<Document> {
    Document::load(path).map_err(|e| match e {
        lopdf::Error::DictKey(_) | lopdf::Error::ObjectNotFound(_) => {
            "檔案結構損毀，請先用「修復」處理".to_string()
        }
        other => format!("無法讀取 PDF：{other}"),
    })
}

fn save(doc: &mut Document, output: &Path) -> Result<()> {
    doc.compress();
    doc.save(output).map(|_| ()).map_err(|e| format!("無法寫出 PDF：{e}"))
}

/// 取得 catalog 底下的 /Pages 根節點。
fn pages_root(doc: &Document) -> Result<ObjectId> {
    doc.catalog()
        .map_err(|e| format!("PDF 缺少 catalog：{e}"))?
        .get(b"Pages")
        .and_then(Object::as_reference)
        .map_err(|e| format!("PDF 的頁面樹異常：{e}"))
}

/// 把頁面樹壓平成單層，/Kids 依 `order` 給定的順序排列。
///
/// 合併、選頁、移除、重排最後都收斂到這個動作——差別只在 `order` 怎麼算出來。
fn rebuild_page_tree(doc: &mut Document, order: &[ObjectId]) -> Result<()> {
    let root = pages_root(doc)?;

    // 每一頁的 /Parent 都要指回這個唯一的根，否則部分閱讀器會拒絕開啟。
    for id in order {
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut(*id) {
            page.set("Parent", root);
        }
    }

    let kids: Vec<Object> = order.iter().map(|id| Object::Reference(*id)).collect();
    let count = kids.len() as i64;
    let Ok(Object::Dictionary(node)) = doc.get_object_mut(root) else {
        return Err("PDF 的頁面樹異常".into());
    };
    node.set("Kids", kids);
    node.set("Count", count);

    // 被移掉的頁面連同它們獨佔的資源會變成孤兒，不清掉檔案不會變小。
    doc.prune_objects();
    Ok(())
}

/// 頁數。UI 需要它來顯示頁數並驗證使用者輸入的頁碼範圍。
pub fn count(input: &Path) -> Result<usize> {
    Ok(load(input)?.get_pages().len())
}

/// 依 1-based 頁碼挑出頁面，並依給定順序輸出。
///
/// 擷取頁面、移除頁面、重新排序都是這一個函式——呼叫端負責算出 `order`。
pub fn select(input: &Path, order: &[u32], output: &Path) -> Result<()> {
    if order.is_empty() {
        return Err("至少要保留一頁".into());
    }
    let mut doc = load(input)?;
    let pages = doc.get_pages();

    let ids = order
        .iter()
        .map(|n| {
            pages
                .get(n)
                .copied()
                .ok_or_else(|| format!("第 {n} 頁不存在（本檔共 {} 頁）", pages.len()))
        })
        .collect::<Result<Vec<_>>>()?;

    rebuild_page_tree(&mut doc, &ids)?;
    save(&mut doc, output)
}

/// 四邊各自往內縮的距離，單位 pt（1/72 吋）。
#[derive(Clone, Copy, Debug, serde::Deserialize)]
pub struct Margins {
    pub left: f64,
    pub bottom: f64,
    pub right: f64,
    pub top: f64,
}

/// 把 PDF 的矩形陣列讀成 [llx, lly, urx, ury]。座標可能是整數或浮點，也可能是間接參照。
fn as_rect(doc: &Document, obj: &Object) -> Option<[f64; 4]> {
    let arr = doc.dereference(obj).ok()?.1.as_array().ok()?;
    if arr.len() != 4 {
        return None;
    }
    let mut out = [0.0f64; 4];
    for (i, item) in arr.iter().enumerate() {
        let v = doc.dereference(item).ok()?.1;
        out[i] = v
            .as_float()
            .map(|f| f as f64)
            .or_else(|_| v.as_i64().map(|n| n as f64))
            .ok()?;
    }
    // PDF 允許左下/右上顛倒，先正規化免得後面算出負寬高。
    Some([
        out[0].min(out[2]),
        out[1].min(out[3]),
        out[0].max(out[2]),
        out[1].max(out[3]),
    ])
}

/// MediaBox 與 CropBox 是可繼承屬性——頁面自己沒有時要往 /Parent 找。
pub fn inherited_rect(doc: &Document, page_id: ObjectId, key: &[u8]) -> Option<[f64; 4]> {
    let mut id = page_id;
    for _ in 0..32 {
        let dict = doc.get_object(id).ok()?.as_dict().ok()?;
        if let Ok(obj) = dict.get(key) {
            if let Some(r) = as_rect(doc, obj) {
                return Some(r);
            }
        }
        id = dict.get(b"Parent").ok()?.as_reference().ok()?;
    }
    None
}

/// 依邊界裁切。以現有的 CropBox 為基準，沒有才退回 MediaBox。
pub fn crop(input: &Path, m: Margins, targets: &[u32], output: &Path) -> Result<()> {
    if [m.left, m.bottom, m.right, m.top].iter().any(|v| *v < 0.0) {
        return Err("邊界不能是負值".into());
    }
    let mut doc = load(input)?;
    let pages = doc.get_pages();

    for n in targets {
        let id = pages
            .get(n)
            .copied()
            .ok_or_else(|| format!("第 {n} 頁不存在（本檔共 {} 頁）", pages.len()))?;

        let base = inherited_rect(&doc, id, b"CropBox")
            .or_else(|| inherited_rect(&doc, id, b"MediaBox"))
            .ok_or_else(|| format!("第 {n} 頁沒有可用的頁面尺寸"))?;

        let new = [
            base[0] + m.left,
            base[1] + m.bottom,
            base[2] - m.right,
            base[3] - m.top,
        ];
        // 裁到沒有面積的話後續閱讀器行為不一致，寧可明確擋掉。
        if new[2] - new[0] < 1.0 || new[3] - new[1] < 1.0 {
            return Err(format!(
                "第 {n} 頁裁切後幾乎沒有內容區（原始尺寸 {:.0}×{:.0} pt）",
                base[2] - base[0],
                base[3] - base[1]
            ));
        }

        if let Ok(Object::Dictionary(page)) = doc.get_object_mut(id) {
            page.set(
                "CropBox",
                Object::Array(new.iter().map(|v| Object::Real(*v as f32)).collect()),
            );
        }
    }
    save(&mut doc, output)
}

/// 依 1-based 頁碼旋轉，`degrees` 為相對角度（90 的倍數）。
pub fn rotate(input: &Path, targets: &[u32], degrees: i64, output: &Path) -> Result<()> {
    if degrees % 90 != 0 {
        return Err("旋轉角度必須是 90 的倍數".into());
    }
    let mut doc = load(input)?;
    let pages = doc.get_pages();

    for n in targets {
        let id = pages
            .get(n)
            .copied()
            .ok_or_else(|| format!("第 {n} 頁不存在（本檔共 {} 頁）", pages.len()))?;
        if let Ok(Object::Dictionary(page)) = doc.get_object_mut(id) {
            let current = page.get(b"Rotate").and_then(Object::as_i64).unwrap_or(0);
            page.set("Rotate", (current + degrees).rem_euclid(360));
        }
    }
    save(&mut doc, output)
}

/// 把 `total` 頁依每份 `chunk` 頁切成連續區段。
///
/// 分開成獨立函式是為了讓呼叫端能先知道會產生幾個檔案——輸出檔在動工前
/// 就要一一建立成受管理的暫存檔，不能等切完才補。
pub fn split_plan(total: usize, chunk: usize) -> Vec<Vec<u32>> {
    if total == 0 || chunk == 0 {
        return Vec::new();
    }
    (1..=total as u32)
        .collect::<Vec<_>>()
        .chunks(chunk)
        .map(<[u32]>::to_vec)
        .collect()
}

/// 依 `plan` 切分成多個檔案。`dests` 的長度必須與 `plan` 相同。
///
/// 只載入一次來源，每份輸出從記憶體中的副本重建——否則 30 頁的掃描件
/// 會被重新解析 30 次。
pub fn split(input: &Path, plan: &[Vec<u32>], dests: &[&Path]) -> Result<()> {
    if plan.len() != dests.len() {
        return Err("內部錯誤：輸出檔數量與切分計畫不符".into());
    }
    let source = load(input)?;
    let pages = source.get_pages();

    for (part, dest) in plan.iter().zip(dests) {
        let ids = part
            .iter()
            .map(|n| {
                pages
                    .get(n)
                    .copied()
                    .ok_or_else(|| format!("第 {n} 頁不存在（本檔共 {} 頁）", pages.len()))
            })
            .collect::<Result<Vec<_>>>()?;

        let mut doc = source.clone();
        rebuild_page_tree(&mut doc, &ids)?;
        save(&mut doc, dest)?;
    }
    Ok(())
}

/// 依序合併多個 PDF。
///
/// lopdf 沒有內建的 merge，得自己搬物件：先把每份文件的物件編號錯開，
/// 再把所有物件併進同一份，最後重建頁面樹。
pub fn merge(inputs: &[&Path], output: &Path) -> Result<()> {
    if inputs.len() < 2 {
        return Err("合併至少需要兩個檔案".into());
    }

    let mut merged = Document::with_version("1.7");
    let mut objects: BTreeMap<ObjectId, Object> = BTreeMap::new();
    let mut order: Vec<ObjectId> = Vec::new();
    let mut next_id = 1u32;

    for path in inputs {
        let mut doc = load(path)?;
        // 錯開編號，否則不同文件的物件會互相覆蓋。
        doc.renumber_objects_with(next_id);
        next_id = doc.max_id + 1;

        // get_pages 依頁碼排序，所以頁序在合併後仍然正確。
        order.extend(doc.get_pages().into_values());
        objects.extend(std::mem::take(&mut doc.objects));
    }

    // 每份來源文件都帶著自己的 Catalog 與 Pages 節點，全部丟掉，稍後重建一組。
    objects.retain(|_, obj| {
        !matches!(
            obj.type_name().unwrap_or_default(),
            b"Catalog" | b"Pages" | b"Outlines"
        )
    });

    merged.objects = objects;
    merged.max_id = next_id;

    let mut pages_node = Dictionary::new();
    pages_node.set("Type", Object::Name(b"Pages".to_vec()));
    pages_node.set("Kids", Object::Array(Vec::new()));
    pages_node.set("Count", Object::Integer(0));
    let pages_id = merged.add_object(pages_node);

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = merged.add_object(catalog);
    merged.trailer.set("Root", catalog_id);

    rebuild_page_tree(&mut merged, &order)?;
    save(&mut merged, output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn corpus(name: &str) -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("bench/corpus")
            .join(name);
        p.exists().then_some(p)
    }

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("localpdf-pages-{name}"))
    }

    fn page_count(path: &Path) -> usize {
        Document::load(path).unwrap().get_pages().len()
    }

    #[test]
    fn select_keeps_requested_pages_in_order() {
        let Some(src) = corpus("c1_text.pdf") else {
            eprintln!("跳過：語料不存在，請先跑 bench/make_corpus.py");
            return;
        };
        let out = tmp("select.pdf");
        select(&src, &[5, 1, 3], &out).unwrap();
        assert_eq!(page_count(&out), 3);
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn select_rejects_out_of_range() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let out = tmp("select-bad.pdf");
        let err = select(&src, &[9999], &out).unwrap_err();
        assert!(err.contains("不存在"), "錯誤訊息應說明頁碼超出範圍：{err}");
    }

    #[test]
    fn removing_pages_shrinks_file() {
        let Some(src) = corpus("c2_scan.pdf") else { return };
        let out = tmp("removed.pdf");
        // 30 頁掃描件只留 3 頁，檔案應該大幅縮小——沒縮小代表孤兒物件沒被清掉。
        select(&src, &[1, 2, 3], &out).unwrap();
        let before = std::fs::metadata(&src).unwrap().len();
        let after = std::fs::metadata(&out).unwrap().len();
        assert!(
            after < before / 5,
            "移除 27/30 頁後檔案只從 {before} 降到 {after}——prune_objects 可能沒生效"
        );
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn rotate_accumulates_and_wraps() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let out = tmp("rotated.pdf");
        rotate(&src, &[1], 270, &out).unwrap();
        let doc = Document::load(&out).unwrap();
        let id = doc.get_pages()[&1];
        let dict = doc.get_object(id).unwrap().as_dict().unwrap();
        assert_eq!(dict.get(b"Rotate").unwrap().as_i64().unwrap(), 270);
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn rotate_rejects_non_right_angles() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        assert!(rotate(&src, &[1], 45, &tmp("x.pdf")).is_err());
    }

    #[test]
    fn crop_shrinks_the_crop_box() {
        let Some(src) = corpus("c1_text.pdf") else {
            eprintln!("跳過：語料不存在，請先跑 bench/make_corpus.py");
            return;
        };
        let out = tmp("crop.pdf");
        let m = Margins { left: 20.0, bottom: 30.0, right: 20.0, top: 30.0 };
        crop(&src, m, &[1], &out).unwrap();

        let doc = Document::load(&out).unwrap();
        let id = doc.get_pages()[&1];
        let cropped = inherited_rect(&doc, id, b"CropBox").unwrap();
        let media = inherited_rect(&doc, id, b"MediaBox").unwrap();

        assert!((cropped[0] - (media[0] + 20.0)).abs() < 0.01, "左邊界應內縮 20pt");
        assert!((cropped[3] - (media[3] - 30.0)).abs() < 0.01, "上邊界應內縮 30pt");
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn crop_rejects_margins_that_eat_the_page() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        // A4 寬約 595pt，左右各切 400pt 會沒有內容區。
        let m = Margins { left: 400.0, bottom: 0.0, right: 400.0, top: 0.0 };
        let err = crop(&src, m, &[1], &tmp("x.pdf")).unwrap_err();
        assert!(err.contains("沒有內容區"), "應說明裁過頭：{err}");
    }

    #[test]
    fn crop_rejects_negative_margins() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let m = Margins { left: -5.0, bottom: 0.0, right: 0.0, top: 0.0 };
        assert!(crop(&src, m, &[1], &tmp("x.pdf")).is_err());
    }

    #[test]
    fn merge_concatenates_page_counts() {
        let (Some(a), Some(b)) = (corpus("c1_text.pdf"), corpus("c3_mixed.pdf")) else {
            eprintln!("跳過：語料不存在");
            return;
        };
        let out = tmp("merged.pdf");
        let expected = page_count(&a) + page_count(&b);
        merge(&[&a, &b], &out).unwrap();
        assert_eq!(page_count(&out), expected);
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn merge_requires_two_files() {
        let Some(a) = corpus("c1_text.pdf") else { return };
        assert!(merge(&[&a], &tmp("x.pdf")).is_err());
    }
}
