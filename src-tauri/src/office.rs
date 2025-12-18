//! Office 文件轉 PDF，透過 LibreOffice 的無介面模式。
//!
//! 這是唯一一個**不隨程式打包**的引擎。LibreOffice 完整安裝約 350 MB，把它塞進
//! 安裝檔會讓體積從 20 MB 變成十幾倍；因此改為偵測系統既有安裝，沒裝就明確告知。
//!
//! 只做 Office → PDF 這個方向。反方向（PDF → Word/Excel）LibreOffice 也做得到，
//! 但它是透過 Draw 模組匯入，產出的是一堆文字方塊拼成的版面，實務上不能用——
//! 與其做一個爛的，不如不做。

use std::path::{Path, PathBuf};
use std::process::Command;

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

type Result<T> = std::result::Result<T, String>;

/// 可以轉的來源格式。
pub const INPUT_EXTENSIONS: &[&str] = &[
    "doc", "docx", "rtf", "odt", "txt", // 文件
    "xls", "xlsx", "ods", "csv", // 試算表
    "ppt", "pptx", "odp", // 簡報
];

pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| INPUT_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

/// 找出 LibreOffice 的執行檔。
///
/// 優先取 `soffice.com` 而不是 `soffice.exe`：在 Windows 上 `.exe` 是視窗版啟動器，
/// 下完命令會立刻返回，我們會在轉檔還沒完成時就去讀輸出檔。`.com` 是主控台版，
/// 會確實等到轉檔結束。
pub fn locate() -> Option<PathBuf> {
    let bases = [
        std::env::var("ProgramFiles").unwrap_or_else(|_| r"C:\Program Files".into()),
        std::env::var("ProgramFiles(x86)").unwrap_or_else(|_| r"C:\Program Files (x86)".into()),
    ];
    for base in bases {
        for exe in ["soffice.com", "soffice.exe"] {
            let p = Path::new(&base).join("LibreOffice/program").join(exe);
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

pub fn version(soffice: &Path) -> Result<String> {
    let out = Command::new(soffice);
    let mut cmd = out;
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let out = cmd
        .arg("--version")
        .output()
        .map_err(|e| format!("無法執行 LibreOffice：{e}"))?;
    let text = String::from_utf8_lossy(&out.stdout);
    Ok(text.trim().lines().next().unwrap_or("LibreOffice").to_string())
}

/// 把 Office 文件轉成 PDF。
///
/// `workdir` 必須位於受管理的暫存根目錄底下——LibreOffice 只能指定輸出資料夾、
/// 不能指定檔名，所以會先落地在這裡，再搬到 `output`。
pub fn to_pdf(soffice: &Path, input: &Path, workdir: &Path, output: &Path) -> Result<()> {
    if !is_supported(input) {
        return Err("這個格式不支援轉檔".into());
    }
    let profile = workdir.join("profile");
    let outdir = workdir.join("out");
    std::fs::create_dir_all(&outdir).map_err(|e| format!("無法建立暫存資料夾：{e}"))?;

    // 用獨立的使用者設定檔，避免使用者自己開著的 LibreOffice 攔截這次轉檔請求
    // ——共用設定檔時，既有實例會接手命令然後我們永遠等不到輸出。
    let profile_url = format!("file:///{}", profile.display().to_string().replace('\\', "/"));

    let mut cmd = Command::new(soffice);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    let result = cmd
        .arg(format!("-env:UserInstallation={profile_url}"))
        .args(["--headless", "--norestore", "--nolockcheck", "--nodefault"])
        .args(["--convert-to", "pdf", "--outdir"])
        .arg(&outdir)
        .arg(input)
        .output()
        .map_err(|e| format!("無法執行 LibreOffice：{e}"))?;

    let produced = std::fs::read_dir(&outdir)
        .map_err(|e| format!("讀不到轉檔結果：{e}"))?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.extension().and_then(|e| e.to_str()) == Some("pdf"));

    let Some(produced) = produced else {
        let err = String::from_utf8_lossy(&result.stderr);
        let hint = err.lines().last().unwrap_or("").trim();
        return Err(if hint.is_empty() {
            "轉檔失敗：LibreOffice 沒有產生輸出，檔案可能已損毀或受密碼保護".into()
        } else {
            format!("轉檔失敗：{hint}")
        });
    };

    std::fs::rename(&produced, output)
        .or_else(|_| std::fs::copy(&produced, output).map(|_| ()))
        .map_err(|e| format!("無法取回轉檔結果：{e}"))?;
    // 這個資料夾裡有 LibreOffice 寫下的中間檔與設定檔，用完立刻整個移除。
    let _ = std::fs::remove_dir_all(workdir);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("localpdf-office-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn extension_filter() {
        assert!(is_supported(Path::new("a.docx")));
        assert!(is_supported(Path::new("A.XLSX")));
        assert!(!is_supported(Path::new("a.pdf")));
        assert!(!is_supported(Path::new("noext")));
    }

    #[test]
    fn converts_a_real_document() {
        let Some(soffice) = locate() else {
            eprintln!("跳過：沒有安裝 LibreOffice");
            return;
        };
        let dir = workdir("convert");
        // 用最單純的來源格式，避免測試相依於另外產生 docx 的工具。
        let src = dir.join("sample.txt");
        std::fs::write(&src, "測試文件\nHello world\n").unwrap();

        let out = dir.join("result.pdf");
        to_pdf(&soffice, &src, &dir.join("work"), &out).unwrap();

        let head = std::fs::read(&out).unwrap();
        assert_eq!(&head[..5], b"%PDF-", "輸出應該是 PDF");
        assert!(
            lopdf::Document::load(&out).unwrap().get_pages().len() >= 1,
            "應該至少有一頁"
        );
    }

    /// 中間檔與 LibreOffice 的設定檔都不該留下來。
    #[test]
    fn cleans_up_its_workdir() {
        let Some(soffice) = locate() else { return };
        let dir = workdir("cleanup");
        let src = dir.join("s.txt");
        std::fs::write(&src, "x").unwrap();
        let work = dir.join("work");

        to_pdf(&soffice, &src, &work, &dir.join("o.pdf")).unwrap();
        assert!(!work.exists(), "轉檔用的暫存資料夾應該被移除");
    }

    #[test]
    fn rejects_unsupported_input() {
        let Some(soffice) = locate() else { return };
        let dir = workdir("reject");
        let src = dir.join("x.pdf");
        std::fs::write(&src, "%PDF-1.7").unwrap();
        assert!(to_pdf(&soffice, &src, &dir.join("w"), &dir.join("o.pdf")).is_err());
    }
}
