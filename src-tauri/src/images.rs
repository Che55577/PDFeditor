//! 圖片轉 PDF。
//!
//! JPEG 的壓縮方式（DCT）本來就是 PDF 原生支援的串流過濾器之一，所以這裡
//! 直接把原始位元組當成 `/DCTDecode` 影像塞進 PDF——不解碼、不重新編碼，
//! 完全無損，而且速度只受限於磁碟。

use std::path::Path;

use lopdf::{Dictionary, Document, Object, Stream};

type Result<T> = std::result::Result<T, String>;

/// A4，單位 pt。
const A4: (f64, f64) = (595.276, 841.890);

struct Jpeg {
    width: u32,
    height: u32,
    components: u8,
}

/// 從 JPEG 的 SOF 標記讀出尺寸與色彩通道數。
///
/// 只掃標記、不解碼影像，所以幾 MB 的照片也是一瞬間。
fn probe(data: &[u8]) -> Result<Jpeg> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err("不是有效的 JPEG 檔案".into());
    }
    let mut i = 2;
    while i + 9 < data.len() {
        if data[i] != 0xFF {
            i += 1;
            continue;
        }
        let marker = data[i + 1];
        // 0xFF 填充位元組，以及不帶長度欄位的獨立標記
        if marker == 0xFF {
            i += 1;
            continue;
        }
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        // SOF0~SOF15 才帶尺寸；其中 C4/C8/CC 分別是 DHT/JPG/DAC，要排除。
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            return Ok(Jpeg {
                height: u16::from_be_bytes([data[i + 5], data[i + 6]]) as u32,
                width: u16::from_be_bytes([data[i + 7], data[i + 8]]) as u32,
                components: data[i + 9],
            });
        }
        let len = u16::from_be_bytes([data[i + 2], data[i + 3]]) as usize;
        if len < 2 {
            return Err("JPEG 結構損毀".into());
        }
        i += 2 + len;
    }
    Err("讀不到 JPEG 的影像尺寸".into())
}

/// 每張圖一頁，依 A4 置中擺放並保持長寬比。
///
/// `margin_pt` 是四邊留白。圖片比頁面大時等比縮小，比頁面小時**不會**放大——
/// 把小圖拉大只會變糊。
pub fn jpegs_to_pdf(inputs: &[&Path], margin_pt: f64, output: &Path) -> Result<()> {
    if inputs.is_empty() {
        return Err("請至少選一張圖片".into());
    }
    if margin_pt < 0.0 {
        return Err("留白不能是負值".into());
    }

    let mut doc = Document::with_version("1.7");
    let pages_id = doc.new_object_id();
    let mut kids: Vec<Object> = Vec::with_capacity(inputs.len());

    for path in inputs {
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let data = std::fs::read(path).map_err(|e| format!("{name}：無法讀取（{e}）"))?;
        let info = probe(&data).map_err(|e| format!("{name}：{e}"))?;

        let color_space = match info.components {
            1 => "DeviceGray",
            3 => "DeviceRGB",
            4 => return Err(format!("{name}：CMYK JPEG 目前不支援，請先轉成 RGB")),
            n => return Err(format!("{name}：無法辨識的色彩格式（{n} 個通道）")),
        };

        // 頁面方向跟著圖片走，橫圖不該被硬塞進直式頁面。
        let (pw, ph) = if info.width > info.height { (A4.1, A4.0) } else { A4 };
        let (avail_w, avail_h) = (pw - margin_pt * 2.0, ph - margin_pt * 2.0);
        if avail_w <= 1.0 || avail_h <= 1.0 {
            return Err("留白太大，頁面沒有可用空間".into());
        }

        // 只縮不放：scale 上限為 1.0。
        let scale = (avail_w / info.width as f64)
            .min(avail_h / info.height as f64)
            .min(1.0);
        let (dw, dh) = (info.width as f64 * scale, info.height as f64 * scale);
        let (dx, dy) = ((pw - dw) / 2.0, (ph - dh) / 2.0);

        let mut image = Dictionary::new();
        image.set("Type", Object::Name(b"XObject".to_vec()));
        image.set("Subtype", Object::Name(b"Image".to_vec()));
        image.set("Width", Object::Integer(info.width as i64));
        image.set("Height", Object::Integer(info.height as i64));
        image.set("ColorSpace", Object::Name(color_space.as_bytes().to_vec()));
        image.set("BitsPerComponent", Object::Integer(8));
        image.set("Filter", Object::Name(b"DCTDecode".to_vec()));
        let image_id = doc.add_object(Stream::new(image, data));

        // cm 是仿射變換：把 1×1 的單位方塊縮放成 dw×dh 並平移到 (dx, dy)。
        let content = format!("q {dw:.3} 0 0 {dh:.3} {dx:.3} {dy:.3} cm /Im Do Q");
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

        let mut xobjects = Dictionary::new();
        xobjects.set("Im", Object::Reference(image_id));
        let mut resources = Dictionary::new();
        resources.set("XObject", Object::Dictionary(xobjects));

        let mut page = Dictionary::new();
        page.set("Type", Object::Name(b"Page".to_vec()));
        page.set("Parent", Object::Reference(pages_id));
        page.set(
            "MediaBox",
            Object::Array(vec![
                Object::Real(0.0),
                Object::Real(0.0),
                Object::Real(pw as f32),
                Object::Real(ph as f32),
            ]),
        );
        page.set("Resources", Object::Dictionary(resources));
        page.set("Contents", Object::Reference(content_id));
        kids.push(Object::Reference(doc.add_object(page)));
    }

    let count = kids.len() as i64;
    let mut pages = Dictionary::new();
    pages.set("Type", Object::Name(b"Pages".to_vec()));
    pages.set("Kids", Object::Array(kids));
    pages.set("Count", Object::Integer(count));
    doc.objects.insert(pages_id, Object::Dictionary(pages));

    let mut catalog = Dictionary::new();
    catalog.set("Type", Object::Name(b"Catalog".to_vec()));
    catalog.set("Pages", Object::Reference(pages_id));
    let catalog_id = doc.add_object(catalog);
    doc.trailer.set("Root", catalog_id);

    // 刻意不呼叫 doc.compress()：影像串流已經是 DCT 壓縮過的，
    // 再包一層 Flate 只會讓檔案變大。
    doc.save(output).map(|_| ()).map_err(|e| format!("無法寫出 PDF：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 用 Ghostscript 從語料算一張 JPEG 出來當測試素材，避免再多一份二進位檔進版控。
    fn sample_jpeg(dir: &Path) -> Option<PathBuf> {
        let gs = crate::gs::locate(None)?;
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("bench/corpus/c3_mixed.pdf");
        if !src.exists() {
            return None;
        }
        std::fs::create_dir_all(dir).ok()?;
        let out = dir.join("sample.jpg");
        crate::gs::render_page(&gs, &src, 1, 72, crate::gs::ImageFormat::Jpeg, &out).ok()?;
        Some(out)
    }

    fn workdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("localpdf-img-{tag}"));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn probe_reads_dimensions() {
        let dir = workdir("probe");
        let Some(jpg) = sample_jpeg(&dir) else {
            eprintln!("跳過：需要 Ghostscript 與語料");
            return;
        };
        let info = probe(&std::fs::read(&jpg).unwrap()).unwrap();
        // A4 直式 @72dpi 約 595×842
        assert!((info.width as i64 - 595).abs() <= 2, "寬度應約 595，實得 {}", info.width);
        assert!((info.height as i64 - 842).abs() <= 2, "高度應約 842，實得 {}", info.height);
        assert_eq!(info.components, 3);
    }

    #[test]
    fn probe_rejects_non_jpeg() {
        assert!(probe(b"%PDF-1.7\n").is_err());
        assert!(probe(&[]).is_err());
    }

    #[test]
    fn one_page_per_image_and_no_reencode() {
        let dir = workdir("topdf");
        let Some(jpg) = sample_jpeg(&dir) else { return };
        let out = dir.join("out.pdf");
        jpegs_to_pdf(&[&jpg, &jpg, &jpg], 0.0, &out).unwrap();

        let doc = Document::load(&out).unwrap();
        assert_eq!(doc.get_pages().len(), 3);

        // 無損嵌入的證據：輸出至少要容納三份原始 JPEG 的位元組。
        let jpeg_bytes = std::fs::metadata(&jpg).unwrap().len();
        let pdf_bytes = std::fs::metadata(&out).unwrap().len();
        assert!(
            pdf_bytes >= jpeg_bytes * 3,
            "輸出 {pdf_bytes} 小於三份原圖 {}，可能被重新編碼了",
            jpeg_bytes * 3
        );
    }

    #[test]
    fn rejects_empty_input_and_negative_margin() {
        let dir = workdir("guard");
        assert!(jpegs_to_pdf(&[], 0.0, &dir.join("x.pdf")).is_err());
        let Some(jpg) = sample_jpeg(&dir) else { return };
        assert!(jpegs_to_pdf(&[&jpg], -1.0, &dir.join("x.pdf")).is_err());
    }
}
