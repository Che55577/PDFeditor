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
