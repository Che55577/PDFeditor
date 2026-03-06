//! 字型內嵌與子集化。
//!
//! 浮水印、頁碼、以及未來的文字註記全部卡在同一件事：把中文字塞進 PDF。
//! 這需要 Type0 複合字型（CIDFontType2 後代 + Identity-H 編碼），而且**必須子集化**
//! ——完整內嵌一份標楷體是 4.9 MB，微軟正黑體是 20 MB，每個輸出檔都背這個重量不可行。
//!
//! 另外一定要產生 ToUnicode CMap，否則從輸出的 PDF 複製文字出來會是亂碼。

use std::collections::BTreeMap;
use std::path::Path;

use lopdf::{Dictionary, Document, Object, ObjectId, Stream};
use subsetter::GlyphRemapper;

type Result<T> = std::result::Result<T, String>;

/// 一段準備好的文字：Identity-H 的十六進位字串，加上以 em 為單位的寬度。
#[derive(Debug)]
pub struct Prepared {
    /// `<hex>` 形式的字串運算元，內容是子集化後的 glyph id。
    pub hex: String,
    /// 寬度，單位是 em。實際寬度 = `width_em * 字級`。
    pub width_em: f64,
}

pub struct Font {
    data: Vec<u8>,
    index: u32,
    name: String,
    units_per_em: f64,
    ascender: f64,
    descender: f64,
    cap_height: f64,
    italic_angle: f64,
    bbox: [f64; 4],
    remapper: GlyphRemapper,
    /// 子集內的新 glyph id → (原始寬度 em 千分位, 對應的字)
    used: BTreeMap<u16, (i64, char)>,
}

impl Font {
    /// 載入字型檔。`index` 是 TTC 集合檔裡的字面編號，一般 TTF 給 0。
    pub fn load(path: &Path, index: u32) -> Result<Self> {
        let name = path.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
        let data = std::fs::read(path).map_err(|e| format!("無法讀取字型檔：{e}"))?;
        let face = ttf_parser::Face::parse(&data, index)
            .map_err(|e| format!("無法解析字型（{name}）：{e}"))?;

        let upem = face.units_per_em() as f64;
        let bbox = face.global_bounding_box();
        let scale = 1000.0 / upem;

        Ok(Self {
            name: name.clone(),
            units_per_em: upem,
            ascender: face.ascender() as f64 * scale,
            descender: face.descender() as f64 * scale,
            cap_height: face.capital_height().unwrap_or(face.ascender()) as f64 * scale,
            italic_angle: face.italic_angle() as f64,
            bbox: [
                bbox.x_min as f64 * scale,
                bbox.y_min as f64 * scale,
                bbox.x_max as f64 * scale,
                bbox.y_max as f64 * scale,
            ],
            data,
            index,
            remapper: GlyphRemapper::new(),
            used: BTreeMap::new(),
        })
    }

    /// 登記一段文字要用到的字，回傳它的編碼與寬度。
    ///
    /// 可以呼叫多次（例如每頁的頁碼不同），所有用到的字會累積進同一個子集。
    pub fn prepare(&mut self, text: &str) -> Result<Prepared> {
        let face = ttf_parser::Face::parse(&self.data, self.index)
            .map_err(|e| format!("無法解析字型：{e}"))?;

        let mut hex = String::with_capacity(text.len() * 4);
        let mut width_units = 0.0f64;

        for ch in text.chars() {
            let gid = face
                .glyph_index(ch)
                .ok_or_else(|| format!("字型「{}」沒有「{ch}」這個字", self.name))?;
            let advance = face.glyph_hor_advance(gid).unwrap_or(0) as f64;
            width_units += advance;

            let new_gid = self.remapper.remap(gid.0);
            self.used.insert(
                new_gid,
                ((advance * 1000.0 / self.units_per_em).round() as i64, ch),
            );
            hex.push_str(&format!("{new_gid:04X}"));
        }

        Ok(Prepared { hex, width_em: width_units / self.units_per_em })
    }

    /// 建立 PDF 的字型物件，回傳可以放進 `/Resources /Font` 的 Type0 字型參照。
    pub fn embed(&self, doc: &mut Document) -> Result<ObjectId> {
        if self.used.is_empty() {
            return Err("沒有任何要嵌入的文字".into());
        }
        let subset = subsetter::subset(&self.data, self.index, &self.remapper)
            .map_err(|e| format!("字型子集化失敗：{e}"))?;

        // PDF 規定子集字型名稱要有六個大寫字母加上加號的前綴。
        let tag = subset_tag(&subset);
        let base_name = format!("{tag}+{}", self.name.replace(' ', ""));

        let mut file_dict = Dictionary::new();
        file_dict.set("Length1", Object::Integer(subset.len() as i64));
        let file_id = doc.add_object(Stream::new(file_dict, subset));

        let mut descriptor = Dictionary::new();
        descriptor.set("Type", name_obj("FontDescriptor"));
        descriptor.set("FontName", name_obj(&base_name));
        // 4 = Symbolic。搭配 Identity-H 時這是最不會出問題的選擇。
        descriptor.set("Flags", Object::Integer(4));
        descriptor.set(
            "FontBBox",
            Object::Array(self.bbox.iter().map(|v| Object::Real(*v as f32)).collect()),
        );
        descriptor.set("ItalicAngle", Object::Real(self.italic_angle as f32));
        descriptor.set("Ascent", Object::Real(self.ascender as f32));
        descriptor.set("Descent", Object::Real(self.descender as f32));
        descriptor.set("CapHeight", Object::Real(self.cap_height as f32));
        // StemV 沒有可靠的來源，給一個保守值；閱讀器只在缺字替代時才會參考它。
        descriptor.set("StemV", Object::Integer(80));
        descriptor.set("FontFile2", Object::Reference(file_id));
        let descriptor_id = doc.add_object(descriptor);

        let mut cid_system = Dictionary::new();
        cid_system.set("Registry", Object::string_literal("Adobe"));
        cid_system.set("Ordering", Object::string_literal("Identity"));
        cid_system.set("Supplement", Object::Integer(0));

        let mut cid_font = Dictionary::new();
        cid_font.set("Type", name_obj("Font"));
        cid_font.set("Subtype", name_obj("CIDFontType2"));
        cid_font.set("BaseFont", name_obj(&base_name));
        cid_font.set("CIDSystemInfo", Object::Dictionary(cid_system));
        cid_font.set("FontDescriptor", Object::Reference(descriptor_id));
        cid_font.set("DW", Object::Integer(1000));
        cid_font.set("W", self.width_array());
        cid_font.set("CIDToGIDMap", name_obj("Identity"));
        let cid_font_id = doc.add_object(cid_font);

        let to_unicode = doc.add_object(Stream::new(Dictionary::new(), self.to_unicode_cmap()));

        let mut type0 = Dictionary::new();
        type0.set("Type", name_obj("Font"));
        type0.set("Subtype", name_obj("Type0"));
        type0.set("BaseFont", name_obj(&base_name));
        type0.set("Encoding", name_obj("Identity-H"));
        type0.set("DescendantFonts", Object::Array(vec![Object::Reference(cid_font_id)]));
        type0.set("ToUnicode", Object::Reference(to_unicode));
        Ok(doc.add_object(type0))
    }

    /// `/W` 陣列，格式為 `[ cid [寬度] cid [寬度] … ]`。
    fn width_array(&self) -> Object {
        let mut out = Vec::with_capacity(self.used.len() * 2);
        for (gid, (width, _)) in &self.used {
            out.push(Object::Integer(*gid as i64));
            out.push(Object::Array(vec![Object::Integer(*width)]));
        }
        Object::Array(out)
    }

    /// ToUnicode CMap。少了它，從輸出的 PDF 複製文字出來會是亂碼。
    fn to_unicode_cmap(&self) -> Vec<u8> {
        let mut body = String::new();
        // bfchar 一段最多 100 筆，超過要分段。
        for chunk in self.used.iter().collect::<Vec<_>>().chunks(100) {
            body.push_str(&format!("{} beginbfchar\n", chunk.len()));
            for (gid, (_, ch)) in chunk {
                let mut utf16 = String::new();
                for unit in ch.encode_utf16(&mut [0u16; 2]).iter() {
                    utf16.push_str(&format!("{unit:04X}"));
                }
                body.push_str(&format!("<{gid:04X}> <{utf16}>\n"));
            }
            body.push_str("endbfchar\n");
        }
        format!(
            "/CIDInit /ProcSet findresource begin\n\
             12 dict begin\nbegincmap\n\
             /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def\n\
             /CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n\
             1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n\
             {body}\
             endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend"
        )
        .into_bytes()
    }
}

fn name_obj(s: &str) -> Object {
    Object::Name(s.as_bytes().to_vec())
}

/// 由子集內容推出一個穩定的六字母標籤——同樣的子集永遠得到同樣的標籤。
fn subset_tag(subset: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for b in subset.iter().take(4096) {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    (0..6)
        .map(|i| (b'A' + ((hash >> (i * 5)) % 26) as u8) as char)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 系統上第一個中文字型（Windows 是標楷體，macOS 是蘋方等）。
    fn test_font() -> Option<crate::platform::FontFace> {
        crate::platform::cjk_test_font()
    }

    fn font() -> Option<Font> {
        test_font().map(|f| Font::load(&f.path, f.index).unwrap())
    }

    #[test]
    fn prepare_encodes_and_measures() {
        let Some(mut f) = font() else {
            eprintln!("跳過：找不到中文字型");
            return;
        };
        let p = f.prepare("測試").unwrap();
        assert_eq!(p.hex.len(), 8, "兩個字應該編成 8 個十六進位字元");
        // 中文是全形，兩個字約等於 2 em。
        assert!((p.width_em - 2.0).abs() < 0.2, "寬度應接近 2 em，實得 {}", p.width_em);
    }

    #[test]
    fn unknown_glyph_is_reported_clearly() {
        let Some(mut f) = font() else { return };
        // 私用區的字，一般字型不會有
        let err = f.prepare("\u{E000}").unwrap_err();
        assert!(err.contains("沒有"), "應說明是缺字：{err}");
    }

    /// 子集化的意義就在這裡：只用幾個字時，內嵌的字型必須遠小於原始檔。
    #[test]
    fn subset_is_far_smaller_than_the_original() {
        let Some(mut f) = font() else { return };
        f.prepare("機密文件").unwrap();

        let original = std::fs::metadata(test_font().unwrap().path).unwrap().len();
        let subset = subsetter::subset(&f.data, f.index, &f.remapper).unwrap();
        assert!(
            (subset.len() as u64) < original / 20,
            "四個字的子集 {} bytes 應該遠小於原始的 {original} bytes",
            subset.len()
        );
    }

    #[test]
    fn embed_builds_a_type0_font() {
        let Some(mut f) = font() else { return };
        f.prepare("第 1 頁").unwrap();

        let mut doc = Document::with_version("1.7");
        let id = f.embed(&mut doc).unwrap();
        let dict = doc.get_object(id).unwrap().as_dict().unwrap();

        assert_eq!(dict.get(b"Subtype").unwrap().as_name().unwrap(), b"Type0");
        assert_eq!(dict.get(b"Encoding").unwrap().as_name().unwrap(), b"Identity-H");
        assert!(dict.get(b"ToUnicode").is_ok(), "少了 ToUnicode 會導致複製出亂碼");
    }

    #[test]
    fn embed_without_text_is_rejected() {
        let Some(f) = font() else { return };
        let mut doc = Document::with_version("1.7");
        assert!(f.embed(&mut doc).is_err());
    }

    #[test]
    fn subset_tag_is_six_uppercase_letters() {
        let tag = subset_tag(b"whatever bytes");
        assert_eq!(tag.len(), 6);
        assert!(tag.chars().all(|c| c.is_ascii_uppercase()));
    }
}
