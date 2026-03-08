//! Python 擴充包。
//!
//! PDF 轉 Word 只有 Python 生態有堪用的實作。實測過的兩條路（見 `docs/ARCHITECTURE.md`）：
//! LibreOffice 產出的是每個視覺行一個浮動文字方塊，改一個字整段版面就散；
//! pdf2docx 產出真段落、零文字方塊，表格偵測在乾淨的數位 PDF 上準確率 100%。
//!
//! 代價是要背 216 MB 的 Python 執行期，所以做成**獨立擴充包**：主安裝檔維持 25 MB，
//! 要用的人才裝。這是一次性下載安裝，不是執行期連網取件，
//! 「處理過程不連網」的宣稱不受影響。

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

type Result<T> = std::result::Result<T, String>;

/// 擴充包最大允許解壓縮總量（600 MB，防止 ZIP 炸彈耗盡磁碟）。
const MAX_PYEXT_TOTAL_BYTES: u64 = 600 * 1024 * 1024;

/// 找出擴充包裡的 Python（Windows：`pyext/python.exe`；macOS/Linux：`pyext/bin/python3`）。
///
/// 依序檢查打包資源、使用者資料目錄（透過介面一鍵匯入）、開發時 vendor/ 與執行檔旁目錄。
pub fn locate(resource_dir: Option<&Path>) -> Option<PathBuf> {
    crate::platform::find_bundled(resource_dir, crate::platform::bin::PYTHON, true)
}

/// 從使用者下載的 `.zip` 擴充包壓縮檔安全解壓並安裝至 `target_root/pyext`。
///
/// 具備 Zip-Slip 路徑穿越攔截、解壓大小上限保護，並在替換正式目錄前先執行 `version()`
/// 驗證 `python + pdf2docx` 確實可用。
pub fn install_archive(archive_path: &Path, target_root: &Path) -> Result<String> {
    let file = std::fs::File::open(archive_path)
        .map_err(|e| format!("無法開啟擴充包壓縮檔：{e}"))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|e| format!("不是有效的 ZIP 擴充包格式：{e}"))?;

    let staging = target_root.join("pyext.installing");
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("無法建立擴充包安裝目錄：{e}"))?;

    let mut total_bytes: u64 = 0;

    let extract_res: Result<()> = (|| {
        for i in 0..zip.len() {
            if crate::is_cancelled() {
                return Err("已取消擴充包安裝".into());
            }
            let mut entry = zip
                .by_index(i)
                .map_err(|e| format!("讀取壓縮包項目失敗：{e}"))?;
            let raw_name = entry.name().to_string();
            if raw_name.split(['/', '\\']).any(|seg| seg == "..") {
                return Err("擴充包內含不安全的相對路徑（已攔截路徑穿越）".into());
            }
            let Some(enclosed) = entry.enclosed_name().map(|p| p.to_path_buf()) else {
                return Err("擴充包內含不安全的相對路徑（已攔截路徑穿越）".into());
            };
            let rel = enclosed
                .strip_prefix("pyext")
                .map(|p| p.to_path_buf())
                .unwrap_or(enclosed);
            if rel.as_os_str().is_empty() {
                continue;
            }

            total_bytes = total_bytes.saturating_add(entry.size());
            if total_bytes > MAX_PYEXT_TOTAL_BYTES {
                return Err("擴充包解壓縮後超過安全大小上限（600 MB）".into());
            }

            let out_path = staging.join(&rel);
            if entry.is_dir() || raw_name.ends_with('/') || raw_name.ends_with('\\') {
                std::fs::create_dir_all(&out_path)
                    .map_err(|e| format!("無法建立目錄：{e}"))?;
                continue;
            }

            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("無法建立父目錄：{e}"))?;
            }

            #[cfg(unix)]
            if (entry.unix_mode().unwrap_or(0) & 0o170000) == 0o120000 {
                let mut link_target = String::new();
                entry
                    .by_ref()
                    .take(4096)
                    .read_to_string(&mut link_target)
                    .map_err(|e| format!("讀取符號連結失敗：{e}"))?;
                let link_target = link_target.trim();
                if link_target.is_empty()
                    || link_target.starts_with('/')
                    || link_target.split(['/', '\\']).any(|seg| seg == "..")
                {
                    return Err("擴充包內含不安全的符號連結目標".into());
                }
                let _ = std::fs::remove_file(&out_path);
                std::os::unix::fs::symlink(link_target, &out_path)
                    .map_err(|e| format!("無法建立符號連結：{e}"))?;
                continue;
            }

            let mut out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("無法寫入擴充包檔案：{e}"))?;
            let copied = std::io::copy(&mut entry.by_ref().take(MAX_PYEXT_TOTAL_BYTES + 1), &mut out_file)
                .map_err(|e| format!("解壓縮寫入失敗：{e}"))?;
            if copied > MAX_PYEXT_TOTAL_BYTES {
                return Err("擴充包單一檔案超過安全大小上限".into());
            }

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = entry.unix_mode().unwrap_or(0o644) & 0o7777;
                let is_bin = rel.starts_with("bin") || rel.extension().is_none();
                let final_mode = if is_bin { mode | 0o755 } else { mode };
                let _ = std::fs::set_permissions(&out_path, std::fs::Permissions::from_mode(final_mode));
            }
        }
        Ok(())
    })();

    if let Err(e) = extract_res {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(e);
    }

    let py_rel = if cfg!(windows) { "python.exe" } else { "bin/python3" };
    let py_bin = staging.join(py_rel);
    if !py_bin.is_file() {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!("壓縮包內找不到預期執行檔（{py_rel}），請確認下載了對應作業系統的 localpdf-pyext 壓縮包"));
    }

    let ver = match version(&py_bin) {
        Ok(v) => v,
        Err(e) => {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    };

    let final_dir = target_root.join("pyext");
    let _ = std::fs::remove_dir_all(&final_dir);
    std::fs::rename(&staging, &final_dir)
        .map_err(|e| format!("無法完成擴充包目錄啟用：{e}"))?;
    Ok(ver)
}

#[allow(unused_mut)]
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
    let mut cmd = build_cmd(py);
    cmd.arg("-c")
        .arg(CONVERT)
        .arg(input)
        .arg(output);
    let result = crate::platform::run_with_timeout(cmd, 180)?;

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

    /// 條碼與 QR 碼在 pdf2docx 的資料模型裡無處可去，若不處理會消失。
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

    #[test]
    fn install_archive_rejects_zip_slip_and_cleans_up() {
        use std::io::Write;
        let dir = workdir("zipslip");
        let zip_path = dir.join("malicious.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("../escaped.txt", opts).unwrap();
            zw.write_all(b"pwned").unwrap();
            zw.finish().unwrap();
        }
        let target = dir.join("userdata");
        let err = install_archive(&zip_path, &target).unwrap_err();
        assert!(err.contains("不安全的相對路徑"), "應攔截路徑穿越：{err}");
        assert!(!dir.join("escaped.txt").exists(), "不該寫出目標目錄之外的檔案");
        assert!(!target.join("pyext.installing").exists(), "暫存安裝目錄應被清除");
    }

    #[test]
    fn install_archive_rejects_archive_without_python_binary() {
        use std::io::Write;
        let dir = workdir("incomplete");
        let zip_path = dir.join("incomplete.zip");
        {
            let file = std::fs::File::create(&zip_path).unwrap();
            let mut zw = zip::ZipWriter::new(file);
            let opts = zip::write::SimpleFileOptions::default();
            zw.start_file("pyext/README.txt", opts).unwrap();
            zw.write_all(b"hello").unwrap();
            zw.finish().unwrap();
        }
        let target = dir.join("userdata");
        let err = install_archive(&zip_path, &target).unwrap_err();
        assert!(err.contains("找不到預期執行檔"), "應回報缺執行檔：{err}");
        assert!(!target.join("pyext.installing").exists(), "暫存安裝目錄應被清除");
    }
}
