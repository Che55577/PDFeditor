//! 影像嵌入與圖片轉 PDF。
//!
//! JPEG 的壓縮方式（DCT）本來就是 PDF 原生支援的串流過濾器，所以直接把原始位元組
//! 當成 `/DCTDecode` 影像塞進去——不解碼、不重新編碼，完全無損。
//!
//! PNG 沒有這個待遇，必須解碼後重新以 Flate 編碼。但 PNG 換來的是**透明度**：
//! 簽名檔與公司章幾乎都帶透明背景，沒有它蓋出來會是一個白方塊糊住文件。
//! 透明度在 PDF 裡是獨立的灰階影像，掛在 `/SMask` 上。

use std::io::Write;
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};

type Result<T> = std::result::Result<T, String>;

/// A4，單位 pt。
const A4: (f64, f64) = (595.276, 841.890);

/// 已經放進文件裡的影像。尺寸是像素，換算成頁面尺寸由呼叫端決定。
pub struct Embedded {
    pub id: ObjectId,
    pub width: u32,
    pub height: u32,
}

/// 把影像檔放進文件，回傳可以掛到 `/XObject` 的參照。
pub fn embed(doc: &mut Document, path: &Path) -> Result<Embedded> {
    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let data = std::fs::read(path).map_err(|e| format!("{name}：無法讀取（{e}）"))?;

    if data.starts_with(&[0xFF, 0xD8]) {
        embed_jpeg(doc, &data, &name)
    } else if data.starts_with(&[0x89, b'P', b'N', b'G']) {
        embed_png(doc, &data, &name)
    } else {
        Err(format!("{name}：只支援 JPEG 與 PNG"))
    }
}

// ---- JPEG ----

struct JpegInfo {
    width: u32,
    height: u32,
    components: u8,
}

/// 從 JPEG 的 SOF 標記讀出尺寸與色彩通道數。只掃標記、不解碼影像。
fn probe_jpeg(data: &[u8]) -> Result<JpegInfo> {
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
        if marker == 0xFF {
            i += 1;
            continue;
        }
        // 不帶長度欄位的獨立標記
        if (0xD0..=0xD9).contains(&marker) || marker == 0x01 {
            i += 2;
            continue;
        }
        // SOF0~SOF15 才帶尺寸；C4/C8/CC 分別是 DHT/JPG/DAC，要排除。
        if (0xC0..=0xCF).contains(&marker) && !matches!(marker, 0xC4 | 0xC8 | 0xCC) {
            return Ok(JpegInfo {
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

fn embed_jpeg(doc: &mut Document, data: &[u8], name: &str) -> Result<Embedded> {
    let info = probe_jpeg(data).map_err(|e| format!("{name}：{e}"))?;
    let color_space = match info.components {
        1 => "DeviceGray",
        3 => "DeviceRGB",
        4 => return Err(format!("{name}：CMYK JPEG 目前不支援，請先轉成 RGB")),
        n => return Err(format!("{name}：無法辨識的色彩格式（{n} 個通道）")),
    };

    let mut dict = base_image_dict(info.width, info.height, color_space);
    dict.set("Filter", Object::Name(b"DCTDecode".to_vec()));
    let id = doc.add_object(Stream::new(dict, data.to_vec()));
    Ok(Embedded { id, width: info.width, height: info.height })
}

// ---- PNG ----

fn embed_png(doc: &mut Document, data: &[u8], name: &str) -> Result<Embedded> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(data));
    // 把調色盤與 16-bit 都攤平成 8-bit 直接色，後面只要處理四種色彩型別。
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder.read_info().map_err(|e| format!("{name}：PNG 解碼失敗（{e}）"))?;

    // 尺寸過大時 png crate 會回 None（緩衝區會溢位），這種檔案本來就不該接受。
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| format!("{name}：PNG 尺寸過大，無法處理"))?;
    let mut buf = vec![0u8; size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| format!("{name}：PNG 影像資料損毀（{e}）"))?;
    let pixels = &buf[..info.buffer_size()];
    let (w, h) = (info.width, info.height);

    let (color, alpha, space): (Vec<u8>, Option<Vec<u8>>, &str) = match info.color_type {
        png::ColorType::Rgb => (pixels.to_vec(), None, "DeviceRGB"),
        png::ColorType::Grayscale => (pixels.to_vec(), None, "DeviceGray"),
        png::ColorType::Rgba => {
            let mut c = Vec::with_capacity(pixels.len() / 4 * 3);
            let mut a = Vec::with_capacity(pixels.len() / 4);
            for px in pixels.chunks_exact(4) {
                c.extend_from_slice(&px[..3]);
                a.push(px[3]);
            }
            (c, Some(a), "DeviceRGB")
        }
        png::ColorType::GrayscaleAlpha => {
            let mut c = Vec::with_capacity(pixels.len() / 2);
            let mut a = Vec::with_capacity(pixels.len() / 2);
            for px in pixels.chunks_exact(2) {
                c.push(px[0]);
                a.push(px[1]);
            }
            (c, Some(a), "DeviceGray")
        }
        other => return Err(format!("{name}：不支援的 PNG 色彩型別（{other:?}）")),
    };

    let mut dict = base_image_dict(w, h, space);
    dict.set("Filter", Object::Name(b"FlateDecode".to_vec()));

    // 透明度在 PDF 裡是獨立的灰階影像，掛在 /SMask 上。
    if let Some(a) = alpha {
        let mut mask = base_image_dict(w, h, "DeviceGray");
        mask.set("Filter", Object::Name(b"FlateDecode".to_vec()));
        let mask_id = doc.add_object(Stream::new(mask, deflate(&a)?));
        dict.set("SMask", Object::Reference(mask_id));
    }

    let id = doc.add_object(Stream::new(dict, deflate(&color)?));
    Ok(Embedded { id, width: w, height: h })
}

fn deflate(raw: &[u8]) -> Result<Vec<u8>> {
    let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(raw).map_err(|e| format!("影像壓縮失敗：{e}"))?;
    enc.finish().map_err(|e| format!("影像壓縮失敗：{e}"))
}

fn base_image_dict(width: u32, height: u32, color_space: &str) -> Dictionary {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"XObject".to_vec()));
    d.set("Subtype", Object::Name(b"Image".to_vec()));
    d.set("Width", Object::Integer(width as i64));
    d.set("Height", Object::Integer(height as i64));
    d.set("ColorSpace", Object::Name(color_space.as_bytes().to_vec()));
    d.set("BitsPerComponent", Object::Integer(8));
    d
}

// ---- 圖片轉 PDF ----

/// 每張圖一頁，依 A4 置中擺放並保持長寬比。
///
/// `margin_pt` 是四邊留白。圖片比頁面大時等比縮小，比頁面小時**不會**放大——
/// 把小圖拉大只會變糊。
pub fn images_to_pdf(inputs: &[&Path], margin_pt: f64, output: &Path) -> Result<()> {
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
        let img = embed(&mut doc, path)?;

        // 頁面方向跟著圖片走，橫圖不該被硬塞進直式頁面。
        let (pw, ph) = if img.width > img.height { (A4.1, A4.0) } else { A4 };
        let (avail_w, avail_h) = (pw - margin_pt * 2.0, ph - margin_pt * 2.0);
        if avail_w <= 1.0 || avail_h <= 1.0 {
            return Err("留白太大，頁面沒有可用空間".into());
        }

        // 只縮不放
        let scale = (avail_w / img.width as f64)
            .min(avail_h / img.height as f64)
            .min(1.0);
        let (dw, dh) = (img.width as f64 * scale, img.height as f64 * scale);
        let (dx, dy) = ((pw - dw) / 2.0, (ph - dh) / 2.0);

        let content = format!("q {dw:.3} 0 0 {dh:.3} {dx:.3} {dy:.3} cm /Im Do Q");
        let content_id = doc.add_object(Stream::new(Dictionary::new(), content.into_bytes()));

        let mut xobjects = Dictionary::new();
        xobjects.set("Im", Object::Reference(img.id));
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

    // 刻意不呼叫 doc.compress()：影像串流已經壓過了，再包一層 Flate 只會變大。
    doc.save(output).map(|_| ()).map_err(|e| format!("無法寫出 PDF：{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn workdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("localpdf-img-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    /// 用 Ghostscript 從語料算一張 JPEG，避免多一份二進位檔進版控。
    fn sample_jpeg(dir: &Path) -> Option<PathBuf> {
        let gs = crate::gs::locate(None)?;
        let src = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("bench/corpus/c3_mixed.pdf");
        if !src.exists() {
            return None;
        }
        let out = dir.join("sample.jpg");
        crate::gs::render_page(&gs, &src, 1, 72, crate::gs::ImageFormat::Jpeg, &out).ok()?;
        Some(out)
    }

    /// 手寫一張帶透明度的 PNG：左半不透明紅、右半全透明。
    fn sample_rgba_png(dir: &Path) -> PathBuf {
        let (w, h) = (40u32, 20u32);
        let mut raw = Vec::with_capacity((w * h * 4) as usize);
        for _y in 0..h {
            for x in 0..w {
                let opaque = x < w / 2;
                raw.extend_from_slice(&[255, 0, 0, if opaque { 255 } else { 0 }]);
            }
        }
        let path = dir.join("alpha.png");
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&raw).unwrap();
        path
    }

    #[test]
    fn probe_reads_jpeg_dimensions() {
        let dir = workdir("probe");
        let Some(jpg) = sample_jpeg(&dir) else {
            eprintln!("跳過：需要 Ghostscript 與語料");
            return;
        };
        let info = probe_jpeg(&std::fs::read(&jpg).unwrap()).unwrap();
        assert!((info.width as i64 - 595).abs() <= 2, "寬度應約 595，實得 {}", info.width);
        assert_eq!(info.components, 3);
    }

    #[test]
    fn rejects_non_image() {
        assert!(probe_jpeg(b"%PDF-1.7\n").is_err());
        assert!(probe_jpeg(&[]).is_err());
        let mut doc = Document::with_version("1.7");
        let dir = workdir("bad");
        let bad = dir.join("x.pdf");
        std::fs::write(&bad, b"%PDF-1.7").unwrap();
        assert!(embed(&mut doc, &bad).is_err());
    }

    /// 透明背景是簽名檔與公司章的必要條件——沒有 SMask 蓋出來就是白方塊。
    #[test]
    fn rgba_png_gets_a_soft_mask() {
        let dir = workdir("smask");
        let png_path = sample_rgba_png(&dir);
        let mut doc = Document::with_version("1.7");
        let img = embed(&mut doc, &png_path).unwrap();

        assert_eq!((img.width, img.height), (40, 20));
        let dict = match doc.get_object(img.id).unwrap() {
            Object::Stream(s) => &s.dict,
            _ => panic!("影像應該是 stream"),
        };
        let mask = dict.get(b"SMask").expect("帶 alpha 的 PNG 必須產生 SMask");
        let mask_id = mask.as_reference().unwrap();
        let mask_dict = match doc.get_object(mask_id).unwrap() {
            Object::Stream(s) => &s.dict,
            _ => panic!("SMask 應該是 stream"),
        };
        assert_eq!(mask_dict.get(b"ColorSpace").unwrap().as_name().unwrap(), b"DeviceGray");
    }

    #[test]
    fn opaque_png_has_no_soft_mask() {
        let dir = workdir("nosmask");
        let (w, h) = (8u32, 8u32);
        let path = dir.join("rgb.png");
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&vec![128u8; (w * h * 3) as usize]).unwrap();

        let mut doc = Document::with_version("1.7");
        let img = embed(&mut doc, &path).unwrap();
        let dict = match doc.get_object(img.id).unwrap() {
            Object::Stream(s) => &s.dict,
            _ => panic!(),
        };
        assert!(dict.get(b"SMask").is_err(), "沒有 alpha 就不該有 SMask");
    }

    #[test]
    fn one_page_per_image_and_jpeg_is_not_reencoded() {
        let dir = workdir("topdf");
        let Some(jpg) = sample_jpeg(&dir) else { return };
        let out = dir.join("out.pdf");
        images_to_pdf(&[&jpg, &jpg, &jpg], 0.0, &out).unwrap();

        assert_eq!(Document::load(&out).unwrap().get_pages().len(), 3);
        let jpeg_bytes = std::fs::metadata(&jpg).unwrap().len();
        assert!(
            std::fs::metadata(&out).unwrap().len() >= jpeg_bytes * 3,
            "輸出小於三份原圖，JPEG 可能被重新編碼了"
        );
    }

    #[test]
    fn mixed_jpeg_and_png_in_one_pdf() {
        let dir = workdir("mixed");
        let Some(jpg) = sample_jpeg(&dir) else { return };
        let png_path = sample_rgba_png(&dir);
        let out = dir.join("mixed.pdf");
        images_to_pdf(&[&jpg, &png_path], 0.0, &out).unwrap();
        assert_eq!(Document::load(&out).unwrap().get_pages().len(), 2);
    }

    #[test]
    fn rejects_empty_input_and_negative_margin() {
        let dir = workdir("guard");
        assert!(images_to_pdf(&[], 0.0, &dir.join("x.pdf")).is_err());
        let png_path = sample_rgba_png(&dir);
        assert!(images_to_pdf(&[&png_path], -1.0, &dir.join("x.pdf")).is_err());
    }
}
