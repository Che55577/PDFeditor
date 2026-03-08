//! PDF 加密與解密，透過 qpdf。
//!
//! 為什麼不用手上既有的引擎：lopdf 的加密支援不完整；Ghostscript 的 pdfwrite 會整份
//! 重寫文件（慢，而且會丟掉書籤與連結）；MuPDF 只有 WASM 版，用它就得把檔案內容
//! 讀進 WebView，為了一個批次工具不值得。qpdf 是 Apache-2.0、8 MB，且加解密是它的本業。
//!
//! ## 密碼絕對不放在命令列
//!
//! 命令列參數在行程清單裡是公開的——同一台機器的其他使用者用工作管理員就看得到。
//! 所以走 qpdf 的 `@-`：參數從 stdin 一行一個讀進去，密碼既不進行程清單也不落地。

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

type Result<T> = std::result::Result<T, String>;

pub fn locate(resource_dir: Option<&Path>) -> Option<PathBuf> {
    let bundled = crate::platform::find_bundled(resource_dir, crate::platform::bin::QPDF, false);
    #[cfg(not(windows))]
    let bundled = bundled.or_else(|| crate::platform::find_on_system("qpdf"));
    bundled
}

pub fn version(qpdf: &Path) -> Result<String> {
    let out = run(qpdf, &["--version".into()])?;
    Ok(out.lines().next().unwrap_or("qpdf").trim().to_string())
}

/// 執行 qpdf，參數一律從 stdin 傳。回傳 stdout。
fn run(qpdf: &Path, args: &[String]) -> Result<String> {
    for arg in args {
        if arg.contains('\n') || arg.contains('\r') {
            return Err("參數不能包含換行字元".into());
        }
    }

    let mut cmd = Command::new(qpdf);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.arg("@-");

    // 一行一個參數，這是 qpdf 對 @- 的約定
    let joined = args.join("\n");
    let out = crate::platform::run_with_timeout_stdin(cmd, Some(joined.as_bytes()), 60)
        .map_err(|e| format!("qpdf 執行失敗：{e}"))?;

    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let msg = err.lines().next_back().unwrap_or("未知錯誤").trim();
        // qpdf 的訊息會帶完整路徑，對使用者沒意義又會洩漏位置
        let msg = msg.rsplit_once(": ").map(|(_, m)| m).unwrap_or(msg);
        return Err(translate(msg));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// 把 qpdf 的英文訊息換成看得懂的說法。
fn translate(msg: &str) -> String {
    let lower = msg.to_lowercase();
    if lower.contains("invalid password") {
        "密碼不正確".into()
    } else if lower.contains("not encrypted") {
        "這個檔案沒有加密，不需要解密".into()
    } else if lower.contains("damaged") || lower.contains("can't find") {
        "檔案結構損毀，請先用「修復」處理".into()
    } else {
        format!("處理失敗：{msg}")
    }
}

/// 加上密碼保護，一律使用 AES-256。
///
/// `user` 是開啟檔案要輸入的密碼；`owner` 管的是列印、複製這類權限。
/// 兩者留空的話 qpdf 會產生一個誰都能開的「加密」檔，沒有意義，所以擋掉。
pub fn encrypt(qpdf: &Path, input: &Path, output: &Path, user: &str, owner: &str) -> Result<()> {
    if user.is_empty() && owner.is_empty() {
        return Err("請至少設定一個密碼".into());
    }
    if user.contains('\n') || user.contains('\r') || owner.contains('\n') || owner.contains('\r') {
        return Err("密碼不能包含換行字元".into());
    }
    // 沒指定擁有者密碼時沿用使用者密碼——留空等於任何人都能移除保護。
    let owner = if owner.is_empty() { user } else { owner };

    run(
        qpdf,
        &[
            "--encrypt".into(),
            format!("--user-password={user}"),
            format!("--owner-password={owner}"),
            "--bits=256".into(),
            "--".into(),
            input.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
        ],
    )?;
    Ok(())
}

/// 移除密碼保護。**需要使用者提供正確密碼**，不做任何破解。
pub fn decrypt(qpdf: &Path, input: &Path, output: &Path, password: &str) -> Result<()> {
    if password.contains('\n') || password.contains('\r') {
        return Err("密碼不能包含換行字元".into());
    }
    run(
        qpdf,
        &[
            format!("--password={password}"),
            "--decrypt".into(),
            "--".into(),
            input.to_string_lossy().into_owned(),
            output.to_string_lossy().into_owned(),
        ],
    )?;
    Ok(())
}

/// 檔案有沒有加密。qpdf 用退出碼回答：0 有、2 沒有。
pub fn is_encrypted(qpdf: &Path, input: &Path) -> bool {
    let mut cmd = Command::new(qpdf);
    #[cfg(windows)]
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd.arg("--is-encrypted")
        .arg(input)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workdir(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("localpdf-crypt-{tag}"));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn corpus() -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()?
            .join("bench/corpus/c1_text.pdf");
        p.exists().then_some(p)
    }

    #[test]
    fn round_trip_encrypt_then_decrypt() {
        let (Some(qpdf), Some(src)) = (locate(None), corpus()) else {
            eprintln!("跳過：沒有 qpdf 或語料");
            return;
        };
        let dir = workdir("roundtrip");
        let enc = dir.join("enc.pdf");
        let dec = dir.join("dec.pdf");

        encrypt(&qpdf, &src, &enc, "hunter2", "").unwrap();
        assert!(is_encrypted(&qpdf, &enc), "加密後應該偵測得到");

        decrypt(&qpdf, &enc, &dec, "hunter2").unwrap();
        assert!(!is_encrypted(&qpdf, &dec), "解密後不該再有加密");
        assert_eq!(
            crate::pages::count(&dec).unwrap(),
            crate::pages::count(&src).unwrap(),
            "頁數不該變"
        );
    }

    #[test]
    fn wrong_password_says_so() {
        let (Some(qpdf), Some(src)) = (locate(None), corpus()) else { return };
        let dir = workdir("wrongpw");
        let enc = dir.join("enc.pdf");
        encrypt(&qpdf, &src, &enc, "correct", "").unwrap();

        let err = decrypt(&qpdf, &enc, &dir.join("x.pdf"), "wrong").unwrap_err();
        assert_eq!(err, "密碼不正確", "錯誤訊息要說人話，實得：{err}");
    }

    #[test]
    fn refuses_empty_password() {
        let (Some(qpdf), Some(src)) = (locate(None), corpus()) else { return };
        let dir = workdir("empty");
        let err = encrypt(&qpdf, &src, &dir.join("x.pdf"), "", "").unwrap_err();
        assert!(err.contains("至少設定一個密碼"), "{err}");
    }

    /// 沒給擁有者密碼時要沿用使用者密碼——留空等於任何人都能移除保護。
    #[test]
    fn blank_owner_password_falls_back_to_user() {
        let (Some(qpdf), Some(src)) = (locate(None), corpus()) else { return };
        let dir = workdir("owner");
        let enc = dir.join("enc.pdf");
        encrypt(&qpdf, &src, &enc, "secret", "").unwrap();
        // 空密碼不該打得開
        assert!(decrypt(&qpdf, &enc, &dir.join("a.pdf"), "").is_err());
        assert!(decrypt(&qpdf, &enc, &dir.join("b.pdf"), "secret").is_ok());
    }

    #[test]
    fn plain_file_is_not_reported_as_encrypted() {
        let (Some(qpdf), Some(src)) = (locate(None), corpus()) else { return };
        assert!(!is_encrypted(&qpdf, &src));
    }
}
