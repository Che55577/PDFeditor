//! Headless CLI 批次命令列模式（`localpdf <subcommand> ...`）。
//!
//! 讓開源開發者與自動化腳本不必啟動 GUI 視窗，即可在終端機或 CI/CD 管線中
//! 直接呼叫 LocalPDF 的核心引擎（合併、選頁重排、移除、旋轉、裁切、分割、壓縮、修復、轉 Markdown、加解密）。

use std::path::{Path, PathBuf};

use crate::{crypt, ephemeral, gs, markdown, pages};

/// 判斷命令列參數是否為 CLI 子指令模式。
///
/// 若使用者直接雙擊圖示啟動（無參數，或僅有作業系統傳入的 `-psn_*` 參數），
/// 回傳 `false` 並進入 Tauri GUI 模式。
pub fn should_run_cli(args: &[String]) -> bool {
    if args.len() <= 1 {
        return false;
    }
    let first = args[1].as_str();
    if first.starts_with("-psn_") {
        return false;
    }
    matches!(
        first,
        "help"
            | "--help"
            | "-h"
            | "version"
            | "--version"
            | "-V"
            | "merge"
            | "extract"
            | "remove"
            | "rotate"
            | "crop"
            | "split"
            | "compress"
            | "repair"
            | "to-markdown"
            | "encrypt"
            | "decrypt"
    )
}

pub const HELP_TEXT: &str = "\
LocalPDF Headless CLI — 本機離線 PDF 批次處理工具

用法 (Usage):
  localpdf <command> [options]

核心指令 (Commands):
  merge <in1.pdf> <in2.pdf...> -o <out.pdf>
      依序合併多份 PDF（保留書籤、連結與表單欄位）

  extract <in.pdf> --pages <spec> -o <out.pdf>
      擷取並依指定順序重排頁面（例如 --pages 3,1,2 或 1-3,7）

  remove <in.pdf> --pages <spec> -o <out.pdf>
      移除指定頁面並清理孤兒物件（例如 --pages 2,5-6）

  rotate <in.pdf> --degrees <90|180|270> [--pages <spec>] -o <out.pdf>
      旋轉指定頁面（未指定 --pages 時旋轉整份文件）

  crop <in.pdf> --margins <top,right,bottom,left> [--pages <spec>] -o <out.pdf>
      依四邊公釐 (mm) 往內裁切頁面（例如 --margins 10,10,10,10）

  split <in.pdf> --chunk <N> -o <out_dir>
      每 N 頁切分為一個獨立 PDF 並輸出至指定資料夾

  compress <in.pdf> [--level extreme|recommended|low] -o <out.pdf>
      使用 Ghostscript 壓縮 PDF（預設 recommended）

  repair <in.pdf> -o <out.pdf>
      重建損毀的交叉參照表 (xref) 與頁面結構

  to-markdown <in.pdf> -o <out.md>
      抽取 PDF 文字並重組段落為 Markdown

  encrypt <in.pdf> --user-pw <pw> [--owner-pw <pw>] -o <out.pdf>
      以 AES-256 加密 PDF

  decrypt <in.pdf> --password <pw> -o <out.pdf>
      提供正確密碼以移除 PDF 密碼保護

通用選項 (Options):
  -h, --help       顯示此說明訊息
  -V, --version    顯示版本資訊
";

/// 解析頁碼規格字串（如 `"3, 1, 5-7"`），**保留使用者指定的順序**並去除重複項。
pub fn parse_pages_spec(spec: &str, max: usize) -> Result<Vec<u32>, String> {
    let mut out: Vec<u32> = Vec::new();
    for raw in spec.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some((a_str, b_str)) = raw.split_once('-') {
            let a: u32 = a_str
                .trim()
                .parse()
                .map_err(|_| format!("頁碼範圍不正確：{raw}"))?;
            let b: u32 = b_str
                .trim()
                .parse()
                .map_err(|_| format!("頁碼範圍不正確：{raw}"))?;
            if a < 1 || b < a {
                return Err(format!("頁碼範圍不正確：{raw}"));
            }
            if b as usize > max {
                return Err(format!("第 {b} 頁不存在（本檔共 {max} 頁）"));
            }
            for p in a..=b {
                if !out.contains(&p) {
                    out.push(p);
                }
            }
        } else {
            let n: u32 = raw
                .parse()
                .map_err(|_| format!("看不懂的頁碼寫法：{raw}"))?;
            if n < 1 {
                return Err(format!("頁碼不正確：{raw}"));
            }
            if n as usize > max {
                return Err(format!("第 {n} 頁不存在（本檔共 {max} 頁）"));
            }
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    if out.is_empty() {
        return Err("請至少指定一頁".into());
    }
    Ok(out)
}

/// 執行 CLI 指令，成功回傳說明訊息，失敗回傳錯誤原因。
pub fn run_cli_args(args: &[String]) -> Result<String, String> {
    if args.len() <= 1 {
        return Ok(HELP_TEXT.to_string());
    }
    let sub = args[1].as_str();
    match sub {
        "help" | "--help" | "-h" => Ok(HELP_TEXT.to_string()),
        "version" | "--version" | "-V" => Ok(format!("LocalPDF {}", env!("CARGO_PKG_VERSION"))),
        "merge" => cmd_merge(&args[2..]),
        "extract" => cmd_extract(&args[2..]),
        "remove" => cmd_remove(&args[2..]),
        "rotate" => cmd_rotate(&args[2..]),
        "crop" => cmd_crop(&args[2..]),
        "split" => cmd_split(&args[2..]),
        "compress" => cmd_compress(&args[2..]),
        "repair" => cmd_repair(&args[2..]),
        "to-markdown" => cmd_to_markdown(&args[2..]),
        "encrypt" => cmd_encrypt(&args[2..]),
        "decrypt" => cmd_decrypt(&args[2..]),
        other => Err(format!("未知的子指令：{other}\n請執行 `localpdf --help` 查看用法。")),
    }
}

fn take_opt(args: &[String], flag: &str) -> Result<(Option<String>, Vec<String>), String> {
    let mut val: Option<String> = None;
    let mut rest: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == flag {
            if i + 1 >= args.len() {
                return Err(format!("選項 {flag} 缺少參數值"));
            }
            val = Some(args[i + 1].clone());
            i += 2;
        } else {
            rest.push(args[i].clone());
            i += 1;
        }
    }
    Ok((val, rest))
}

fn cmd_merge(args: &[String]) -> Result<String, String> {
    let (out, inputs) = take_opt(args, "-o")?;
    let out = out.ok_or("請使用 `-o <out.pdf>` 指定輸出檔案路徑")?;
    if inputs.len() < 2 {
        return Err("`merge` 至少需要兩個輸入 PDF 檔案".into());
    }
    let paths: Vec<PathBuf> = inputs.iter().map(PathBuf::from).collect();
    let refs: Vec<&Path> = paths.iter().map(|p| p.as_path()).collect();
    pages::merge(&refs, Path::new(&out))?;
    Ok(format!("已合併 {} 個檔案至 {out}", inputs.len()))
}

fn cmd_extract(args: &[String]) -> Result<String, String> {
    let (out, rem) = take_opt(args, "-o")?;
    let (spec, inputs) = take_opt(&rem, "--pages")?;
    let out = out.ok_or("請使用 `-o <out.pdf>` 指定輸出檔案路徑")?;
    let spec = spec.ok_or("請使用 `--pages <頁碼>` 指定要保留與重排的頁碼（例如 3,1,2）")?;
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let src_path = Path::new(src);
    let max = pages::count(src_path)?;
    let order = parse_pages_spec(&spec, max)?;
    pages::select(src_path, &order, Path::new(&out))?;
    Ok(format!("已擷取 {} 頁至 {out}", order.len()))
}

fn cmd_remove(args: &[String]) -> Result<String, String> {
    let (out, rem) = take_opt(args, "-o")?;
    let (spec, inputs) = take_opt(&rem, "--pages")?;
    let out = out.ok_or("請使用 `-o <out.pdf>` 指定輸出檔案路徑")?;
    let spec = spec.ok_or("請使用 `--pages <頁碼>` 指定要移除的頁碼")?;
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let src_path = Path::new(src);
    let max = pages::count(src_path)?;
    let drop_pages = parse_pages_spec(&spec, max)?;
    let keep: Vec<u32> = (1..=max as u32).filter(|n| !drop_pages.contains(n)).collect();
    if keep.is_empty() {
        return Err("不能移除所有頁面".into());
    }
    pages::select(src_path, &keep, Path::new(&out))?;
    Ok(format!("已移除 {} 頁，輸出至 {out}", drop_pages.len()))
}

fn cmd_rotate(args: &[String]) -> Result<String, String> {
    let (out, rem) = take_opt(args, "-o")?;
    let (deg_str, rem) = take_opt(&rem, "--degrees")?;
    let (spec, inputs) = take_opt(&rem, "--pages")?;
    let out = out.ok_or("請使用 `-o <out.pdf>` 指定輸出檔案路徑")?;
    let degrees: i64 = deg_str
        .ok_or("請使用 `--degrees <90|180|270>` 指定旋轉角度")?
        .parse()
        .map_err(|_| "旋轉角度必須是整數")?;
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let src_path = Path::new(src);
    let max = pages::count(src_path)?;
    let targets = match spec {
        Some(s) if !s.trim().is_empty() => parse_pages_spec(&s, max)?,
        _ => (1..=max as u32).collect(),
    };
    pages::rotate(src_path, &targets, degrees, Path::new(&out))?;
    Ok(format!("已旋轉 {} 頁（{degrees}°），輸出至 {out}", targets.len()))
}

fn cmd_crop(args: &[String]) -> Result<String, String> {
    let (out, rem) = take_opt(args, "-o")?;
    let (margins_str, rem) = take_opt(&rem, "--margins")?;
    let (spec, inputs) = take_opt(&rem, "--pages")?;
    let out = out.ok_or("請使用 `-o <out.pdf>` 指定輸出檔案路徑")?;
    let margins_str = margins_str.ok_or("請使用 `--margins <上,右,下,左>` 指定四邊內縮公釐 (mm)")?;
    let parts: Vec<f64> = margins_str
        .split(',')
        .map(|s| s.trim().parse::<f64>())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| "邊界格式錯誤，請提供 4 個數值（例如 10,10,10,10）")?;
    if parts.len() != 4 {
        return Err("邊界必須包含 4 個數值：上,右,下,左（公釐）".into());
    }
    let mm_to_pt = 72.0 / 25.4;
    let margins = pages::Margins {
        top: parts[0] * mm_to_pt,
        right: parts[1] * mm_to_pt,
        bottom: parts[2] * mm_to_pt,
        left: parts[3] * mm_to_pt,
    };
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let src_path = Path::new(src);
    let max = pages::count(src_path)?;
    let targets = match spec {
        Some(s) if !s.trim().is_empty() => parse_pages_spec(&s, max)?,
        _ => (1..=max as u32).collect(),
    };
    pages::crop(src_path, margins, &targets, Path::new(&out))?;
    Ok(format!("已裁切 {} 頁，輸出至 {out}", targets.len()))
}

fn cmd_split(args: &[String]) -> Result<String, String> {
    let (out_dir, rem) = take_opt(args, "-o")?;
    let (chunk_str, inputs) = take_opt(&rem, "--chunk")?;
    let out_dir = PathBuf::from(out_dir.ok_or("請使用 `-o <out_dir>` 指定輸出資料夾")?);
    let chunk: usize = chunk_str
        .unwrap_or_else(|| "1".into())
        .parse()
        .map_err(|_| "`--chunk` 必須是正整數")?;
    if chunk == 0 {
        return Err("`--chunk` 至少為 1".into());
    }
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let src_path = Path::new(src);
    let total = pages::count(src_path)?;
    let plan = pages::split_plan(total, chunk);
    if plan.len() < 2 {
        return Err("這份文件的頁數不足以依此設定分割".into());
    }
    std::fs::create_dir_all(&out_dir).map_err(|e| format!("無法建立輸出資料夾：{e}"))?;
    let stem = src_path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "split".into());
    let dest_paths: Vec<PathBuf> = plan
        .iter()
        .map(|part| {
            let (first, last) = (part[0], part[part.len() - 1]);
            let name = if first == last {
                format!("{stem}_p{first}.pdf")
            } else {
                format!("{stem}_p{first}-{last}.pdf")
            };
            out_dir.join(name)
        })
        .collect();
    let dest_refs: Vec<&Path> = dest_paths.iter().map(|p| p.as_path()).collect();
    pages::split(src_path, &plan, &dest_refs)?;
    Ok(format!("已分割為 {} 個檔案並儲存至 {}", dest_paths.len(), out_dir.display()))
}

fn cmd_compress(args: &[String]) -> Result<String, String> {
    let (out, rem) = take_opt(args, "-o")?;
    let (level_str, inputs) = take_opt(&rem, "--level")?;
    let out = out.ok_or("請使用 `-o <out.pdf>` 指定輸出檔案路徑")?;
    let level = match level_str.as_deref().unwrap_or("recommended") {
        "extreme" => gs::Level::Extreme,
        "recommended" => gs::Level::Recommended,
        "low" => gs::Level::Low,
        other => return Err(format!("不支援的壓縮等級：{other}（可用：extreme, recommended, low）")),
    };
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let gs_bin = gs::locate(None).ok_or("找不到 Ghostscript 引擎")?;
    gs::compress(&gs_bin, Path::new(src), Path::new(&out), level)?;
    Ok(format!("已壓縮輸出至 {out}"))
}

fn cmd_repair(args: &[String]) -> Result<String, String> {
    let (out, inputs) = take_opt(args, "-o")?;
    let out = out.ok_or("請使用 `-o <out.pdf>` 指定輸出檔案路徑")?;
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let gs_bin = gs::locate(None).ok_or("找不到 Ghostscript 引擎")?;
    gs::repair(&gs_bin, Path::new(src), Path::new(&out))?;
    Ok(format!("已修復輸出至 {out}"))
}

fn cmd_to_markdown(args: &[String]) -> Result<String, String> {
    let (out, inputs) = take_opt(args, "-o")?;
    let out = out.ok_or("請使用 `-o <out.md>` 指定輸出檔案路徑")?;
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let gs_bin = gs::locate(None).ok_or("找不到 Ghostscript 引擎")?;
    let eph_dir = ephemeral::root(&std::env::temp_dir().join("localpdf-cli"));
    std::fs::create_dir_all(&eph_dir).map_err(|e| format!("無法建立暫存目錄：{e}"))?;
    markdown::pdf_to_markdown(&gs_bin, &eph_dir, Path::new(src), Path::new(&out))?;
    ephemeral::sweep(&eph_dir);
    Ok(format!("已轉換為 Markdown 並輸出至 {out}"))
}

fn cmd_encrypt(args: &[String]) -> Result<String, String> {
    let (out, rem) = take_opt(args, "-o")?;
    let (user_pw, rem) = take_opt(&rem, "--user-pw")?;
    let (owner_pw, inputs) = take_opt(&rem, "--owner-pw")?;
    let out = out.ok_or("請使用 `-o <out.pdf>` 指定輸出檔案路徑")?;
    let user = user_pw.unwrap_or_default();
    let owner = owner_pw.unwrap_or_default();
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let qpdf_bin = crypt::locate(None).ok_or("找不到 qpdf 加密引擎")?;
    crypt::encrypt(&qpdf_bin, Path::new(src), Path::new(&out), &user, &owner)?;
    Ok(format!("已以 AES-256 加密並輸出至 {out}"))
}

fn cmd_decrypt(args: &[String]) -> Result<String, String> {
    let (out, rem) = take_opt(args, "-o")?;
    let (pw, inputs) = take_opt(&rem, "--password")?;
    let out = out.ok_or("請使用 `-o <out.pdf>` 指定輸出檔案路徑")?;
    let password = pw.ok_or("請使用 `--password <pw>` 提供 PDF 密碼")?;
    let src = inputs.first().ok_or("請提供輸入 PDF 檔案路徑")?;
    let qpdf_bin = crypt::locate(None).ok_or("找不到 qpdf 解密引擎")?;
    crypt::decrypt(&qpdf_bin, Path::new(src), Path::new(&out), &password)?;
    Ok(format!("已解密並輸出至 {out}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus(name: &str) -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("bench/corpus")
            .join(name);
        p.exists().then_some(p)
    }

    #[test]
    fn parse_pages_preserves_custom_reordering_and_deduplicates() {
        let order = parse_pages_spec("3, 1, 2, 3, 4-5", 10).unwrap();
        assert_eq!(order, vec![3, 1, 2, 4, 5]);
    }

    #[test]
    fn should_run_cli_detects_subcommands_and_ignores_gui_launch() {
        assert!(!should_run_cli(&["localpdf".into()]));
        assert!(!should_run_cli(&["localpdf".into(), "-psn_0_12345".into()]));
        assert!(should_run_cli(&["localpdf".into(), "--help".into()]));
        assert!(should_run_cli(&["localpdf".into(), "extract".into()]));
        assert!(should_run_cli(&["localpdf".into(), "compress".into()]));
    }

    #[test]
    fn cli_extract_and_rotate_work_end_to_end() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let dir = std::env::temp_dir().join("localpdf-cli-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let extracted = dir.join("extracted.pdf");
        let args = vec![
            "localpdf".to_string(),
            "extract".to_string(),
            src.to_string_lossy().to_string(),
            "--pages".to_string(),
            "3,1".to_string(),
            "-o".to_string(),
            extracted.to_string_lossy().to_string(),
        ];
        let msg = run_cli_args(&args).unwrap();
        assert!(msg.contains("已擷取 2 頁"));
        assert_eq!(pages::count(&extracted).unwrap(), 2);

        let rotated = dir.join("rotated.pdf");
        let args_rot = vec![
            "localpdf".to_string(),
            "rotate".to_string(),
            extracted.to_string_lossy().to_string(),
            "--degrees".to_string(),
            "90".to_string(),
            "--pages".to_string(),
            "1".to_string(),
            "-o".to_string(),
            rotated.to_string_lossy().to_string(),
        ];
        run_cli_args(&args_rot).unwrap();
        assert_eq!(pages::count(&rotated).unwrap(), 2);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
