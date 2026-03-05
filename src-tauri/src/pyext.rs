//! Python 擴充包。
//!
//! PDF 轉 Word 只有 Python 生態有堪用的實作。實測過的兩條路（見 `docs/pdf-to-office.md`）：
//! LibreOffice 產出的是每個視覺行一個浮動文字方塊，改一個字整段版面就散；
//! pdf2docx 產出真段落、零文字方塊，表格偵測在乾淨的數位 PDF 上準確率 100%。
//!
//! 代價是要背 216 MB 的 Python 執行期，所以做成**獨立擴充包**：主安裝檔維持 25 MB，
//! 要用的人才裝。這是一次性下載安裝，不是執行期連網取件，
//! 「處理過程不連網」的宣稱不受影響。

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

type Result<T> = std::result::Result<T, String>;

/// 找出擴充包裡的 Python。
pub fn locate(resource_dir: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(res) = resource_dir {
        candidates.push(res.join("pyext/python.exe"));
    }
    // 開發時：專案根目錄的 vendor/
    if let Ok(exe) = std::env::current_exe() {
        for up in exe.ancestors().take(6) {
            candidates.push(up.join("vendor/pyext/python.exe"));
            // 安裝後擴充包會落在程式目錄底下
            candidates.push(up.join("pyext/python.exe"));
        }
    }
    candidates.into_iter().find(|p| p.exists())
}

fn build_cmd(py: &Path) -> Command {
    let mut cmd = Command::new(py);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

pub fn version(py: &Path) -> Result<String> {
    let out = build_cmd(py)
        .args(["-c", "import pdf2docx,sys;print(f'Python {sys.version_info.major}.{sys.version_info.minor} + pdf2docx')"])
        .output()
        .map_err(|e| format!("無法執行擴充包：{e}"))?;
    if !out.status.success() {
        return Err("擴充包安裝不完整".into());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// 轉檔腳本，和 bench 共用同一份原始碼。
///
/// 內嵌而不是隨擴充包安裝：擴充包 216 MB、更新一次很痛，而腳本會隨著量測結果調整。
/// 綁在執行檔裡就不會出現「app 更新了但擴充包裡還是舊腳本」這種對不起來的組合，
/// 量測與產品也保證跑的是同一段程式。
///
/// 腳本在交給 pdf2docx 之前會把向量密集的區域點陣化——條碼與 QR 碼在 pdf2docx 的
/// 資料模型裡無處可去，不先轉成圖片就會整個消失。詳見 `scripts/pdf2word.py`。
const CONVERT: &str = include_str!("../../scripts/pdf2word.py");

/// PDF 轉 Word。
///
/// 轉完會修一次字型名稱：pdf2docx 寫出的名稱常常在系統上不存在（實測 `DFKaiShu-SB-Estd-BF`
/// 會變成 `DFKai`），對不上就會退回預設字型，字寬一變整段重新換行——那就是「跑版」。
/// 詳見 `docxfont`。
///
/// `output` **必須是 `Ephemeral` 的路徑**：腳本做向量前處理時會在它旁邊開一個
/// `.prep.pdf` 中間檔，硬砍行程時要靠啟動掃除清掉。
pub fn pdf_to_docx(py: &Path, input: &Path, output: &Path, fallback_font: &str) -> Result<()> {
    let result = build_cmd(py)
        .arg("-c")
        .arg(CONVERT)
        .arg(input)
        .arg(output)
        .output()
        .map_err(|e| format!("無法執行轉檔：{e}"))?;

    if !result.status.success() {
        let err = String::from_utf8_lossy(&result.stderr);
        // Python 的 traceback 最後一行才是真正的錯誤
        let msg = err
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.starts_with("  "))
            .next_back()
            .unwrap_or("未知錯誤");
        return Err(format!("轉檔失敗：{}", msg.trim()));
    }
    match std::fs::metadata(output) {
        Ok(m) if m.len() > 0 => {}
        _ => return Err("轉檔失敗：沒有產生輸出，這份 PDF 可能是掃描件（需要先做 OCR）".into()),
    }
    // 字型修不好不該讓整趟轉檔失敗——沒修的檔案仍然可用，只是版面比較不準。
    if let Err(e) = crate::docxfont::remap(output, fallback_font) {
        eprintln!("字型名稱修正失敗，輸出仍可使用：{e}");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("localpdf-pyext-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn corpus(name: &str) -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("bench/corpus")
            .join(name);
        p.exists().then_some(p)
    }

    /// 這是選 pdf2docx 而不是 LibreOffice 的整個理由：產出真段落，不是浮動文字方塊。
    #[test]
    fn converts_to_real_paragraphs_not_text_boxes() {
        let (Some(py), Some(src)) = (locate(None), corpus("c1_text.pdf")) else {
            eprintln!("跳過：沒有安裝擴充包或語料，見 scripts/build-pyext.py");
            return;
        };
        let dir = workdir("convert");
        let out = dir.join("out.docx");
        pdf_to_docx(&py, &src, &out, "標楷體").unwrap();

        let xml = {
            let file = std::fs::File::open(&out).unwrap();
            let mut zip = zip::ZipArchive::new(file).unwrap();
            let mut s = String::new();
            std::io::Read::read_to_string(
                &mut zip.by_name("word/document.xml").unwrap(),
                &mut s,
            )
            .unwrap();
            s
        };
        assert!(xml.contains("壓縮測試"), "中文應該正確保留");
        assert!(
            !xml.contains("<w:txbxContent>"),
            "不該出現浮動文字方塊——那正是 LibreOffice 那條路不能用的原因"
        );
        // pdf2docx 原本會寫出系統上不存在的 DFKai，那是跑版的主因
        assert!(
            !xml.contains(r#"w:eastAsia="DFKai""#),
            "字型名稱應該已被換成實際安裝的字型"
        );
    }

    /// 條碼與 QR 碼在 pdf2docx 的資料模型裡無處可去，會整個消失。
    /// 唯一救得回來的辦法是先點陣化，所以這裡守的是那條前處理路徑。
    #[test]
    fn barcodes_survive_as_images() {
        let (Some(py), Some(src)) = (locate(None), corpus("c4_vector.pdf")) else {
            eprintln!("跳過：沒有安裝擴充包或語料，見 scripts/build-pyext.py");
            return;
        };
        let dir = workdir("vector");
        let out = dir.join("out.docx");
        pdf_to_docx(&py, &src, &out, "標楷體").unwrap();

        let file = std::fs::File::open(&out).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let images = (0..zip.len())
            .filter(|i| zip.by_index(*i).unwrap().name().starts_with("word/media/"))
            .count();
        assert!(images >= 2, "條碼與 QR 應該各留下一張圖，實際只有 {images} 張");

        // 前處理的中間檔開在輸出檔旁邊，不清掉就等於把來源內容留在磁碟上。
        let leftovers: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.ends_with(".prep.pdf"))
            .collect();
        assert!(leftovers.is_empty(), "中間檔沒有清掉：{leftovers:?}");
    }

    #[test]
    fn reports_a_clear_error_for_non_pdf() {
        let Some(py) = locate(None) else { return };
        let dir = workdir("bad");
        let src = dir.join("x.pdf");
        std::fs::write(&src, b"not a pdf at all").unwrap();
        let err = pdf_to_docx(&py, &src, &dir.join("o.docx"), "Arial").unwrap_err();
        assert!(err.contains("轉檔失敗"), "訊息應該說明失敗：{err}");
    }
}
