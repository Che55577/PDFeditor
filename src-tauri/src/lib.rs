mod annotate;
mod crypt;
mod docxfont;
mod ephemeral;
mod font;
mod gs;
mod images;
mod markdown;
mod office;
mod pages;
mod platform;
mod pyext;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use ephemeral::Ephemeral;
use gs::Level;
use tauri::{AppHandle, Emitter, Manager, State};

/// 全域取消旗標，讓前端可隨時中止執行中的批次管線與外部子行程。
static CANCEL_REQUESTED: AtomicBool = AtomicBool::new(false);

pub fn is_cancelled() -> bool {
    CANCEL_REQUESTED.load(Ordering::SeqCst)
}

#[tauri::command]
fn cancel_pipeline() {
    CANCEL_REQUESTED.store(true, Ordering::SeqCst);
}

struct AppState {
    /// 壓縮引擎路徑。找不到時為 None，UI 會顯示引擎缺失而不是讓每次操作都失敗。
    gs: Option<PathBuf>,
    /// LibreOffice。這個不隨程式打包（350 MB 太重），偵測系統既有安裝。
    office: Option<PathBuf>,
    /// Python 擴充包。216 MB，做成獨立安裝檔，沒裝屬正常情況。
    pyext: Option<PathBuf>,
    /// qpdf，隨程式打包（8 MB），負責加解密。
    qpdf: Option<PathBuf>,
    /// 所有落地的暫存檔都只會出現在這個目錄下。
    eph_root: PathBuf,
    /// 已完成、等待使用者儲存的結果。從這裡移除即代表抹除。
    jobs: Mutex<HashMap<String, Ephemeral>>,
}

/// 使用者要執行的操作。前端以 `{ kind: "...", ... }` 傳入。
///
/// 除了 Merge 是「多進一出」，其餘都是「每個檔案各自處理」。
#[derive(serde::Deserialize, Clone)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum Operation {
    Compress { level: Level },
    Merge,
    /// 只保留這些頁（順序即輸出順序，因此也涵蓋重新排序）
    Extract { pages: Vec<u32> },
    /// 移除這些頁，其餘維持原順序
    Remove { pages: Vec<u32> },
    /// pages 為空代表整份文件
    Rotate { pages: Vec<u32>, degrees: i64 },
    /// pages 為空代表整份文件；margins 單位為 pt
    Crop { pages: Vec<u32>, margins: pages::Margins },
    /// 每 chunk 頁切成一個檔案
    Split { chunk: usize },
    /// 每頁輸出一張圖片
    ToImage { dpi: u32, format: gs::ImageFormat },
    /// 多張 JPEG 併成一份 PDF；margin 單位為 pt
    ImagesToPdf { margin: f64 },
    /// 重建損毀的檔案
    Repair,
    /// 斜向文字浮水印
    Watermark {
        text: String,
        style: annotate::TextStyle,
        angle: f64,
        pages: Vec<u32>,
    },
    /// 頁碼。template 內的 {n} 與 {total} 會被替換
    PageNumbers {
        template: String,
        style: annotate::TextStyle,
        corner: annotate::Corner,
        start_at: i64,
        margin: f64,
        pages: Vec<u32>,
    },
    /// Word / Excel / PowerPoint 轉 PDF
    OfficeToPdf,
    /// PDF 轉 Word，需要 Python 擴充包。
    /// fallback_font 是輸出裡對不上系統字型時要換成什麼——見 docxfont。
    PdfToWord { fallback_font: String },
    /// 抽出文字並整理成 Markdown
    ToMarkdown,
    /// 蓋圖章（簽名檔、公司章）。image 是另外選的圖片檔，不在批次輸入裡。
    Stamp {
        image: String,
        place: annotate::Placement,
        pages: Vec<u32>,
    },
    /// 加上 AES-256 密碼保護
    Encrypt { user: String, owner: String },
    /// 移除密碼保護。**需要使用者提供正確密碼**，不做破解。
    Decrypt { password: String },
}

impl Operation {
    /// 這些是「多進一出」——整批輸入一次處理完，其餘都是每個檔案各跑一次。
    fn merges(&self) -> bool {
        matches!(self, Operation::Merge | Operation::ImagesToPdf { .. })
    }

    /// 需要 Ghostscript 的工具。先擋掉才不會讓每個檔案各失敗一次。
    fn needs_gs(&self) -> bool {
        matches!(
            self,
            Operation::Compress { .. }
                | Operation::ToImage { .. }
                | Operation::Repair
                | Operation::ToMarkdown
        )
    }

    fn needs_office(&self) -> bool {
        matches!(self, Operation::OfficeToPdf)
    }

    fn needs_pyext(&self) -> bool {
        matches!(self, Operation::PdfToWord { .. })
    }

    fn extension(&self) -> &'static str {
        match self {
            Operation::ToImage { format, .. } => format.extension(),
            Operation::ToMarkdown => "md",
            Operation::PdfToWord { .. } => "docx",
            _ => "pdf",
        }
    }

    /// 串接時的執行順序。
    ///
    /// 順序不是隨便定的：解密要最先，加密的檔案什麼都做不了；格式轉換要先把來源變成
    /// PDF；修復要在動結構之前；頁面操作要在加註記之前（否則註記會蓋在被刪掉的頁上）；
    /// 壓縮幾乎一定要最後——先壓再疊圖只會讓疊上去的東西沒被壓到，而且中間步驟重寫
    /// 檔案會抵銷壓縮效果；加密則要排在分割之後，讓每一份都各自受保護。
    fn order(&self) -> u8 {
        match self {
            Operation::OfficeToPdf | Operation::ImagesToPdf { .. } => 0,
            Operation::Decrypt { .. } => 1,
            Operation::Repair => 2,
            Operation::Merge => 3,
            Operation::Extract { .. } | Operation::Remove { .. } => 4,
            Operation::Rotate { .. } => 5,
            Operation::Crop { .. } => 6,
            Operation::Watermark { .. } => 7,
            Operation::PageNumbers { .. } => 8,
            Operation::Stamp { .. } => 9,
            Operation::Compress { .. } => 10,
            Operation::Split { .. } => 11,
            Operation::Encrypt { .. } => 12,
            Operation::ToImage { .. } | Operation::ToMarkdown | Operation::PdfToWord { .. } => 13,
        }
    }

    /// 給使用者看的名稱，錯誤訊息會用到。
    fn label(&self) -> &'static str {
        match self {
            Operation::Compress { .. } => "壓縮",
            Operation::Merge => "合併",
            Operation::Extract { .. } => "擷取頁面",
            Operation::Remove { .. } => "移除頁面",
            Operation::Rotate { .. } => "旋轉",
            Operation::Crop { .. } => "裁切",
            Operation::Split { .. } => "分割",
            Operation::ToImage { .. } => "轉成圖片",
            Operation::ImagesToPdf { .. } => "圖片轉 PDF",
            Operation::Repair => "修復",
            Operation::Watermark { .. } => "浮水印",
            Operation::PageNumbers { .. } => "頁碼",
            Operation::Stamp { .. } => "圖章",
            Operation::OfficeToPdf => "Office 轉 PDF",
            Operation::ToMarkdown => "轉成 Markdown",
            Operation::PdfToWord { .. } => "PDF 轉 Word",
            Operation::Encrypt { .. } => "加密",
            Operation::Decrypt { .. } => "解密",
        }
    }

    fn needs_qpdf(&self) -> bool {
        matches!(self, Operation::Encrypt { .. } | Operation::Decrypt { .. })
    }

    /// 產出非 PDF，後面不能再接任何步驟。
    fn is_terminal(&self) -> bool {
        matches!(
            self,
            Operation::ToImage { .. } | Operation::ToMarkdown | Operation::PdfToWord { .. }
        )
    }

    /// 吃的不是 PDF，只能放在最前面。
    fn is_entry(&self) -> bool {
        matches!(self, Operation::OfficeToPdf | Operation::ImagesToPdf { .. })
    }
}

/// 檢查一組操作能不能串在一起，不行就回傳看得懂的理由。
fn validate_pipeline(ops: &[Operation]) -> Result<(), String> {
    if ops.is_empty() {
        return Err("請至少勾選一項功能".into());
    }
    let entries: Vec<&str> = ops.iter().filter(|o| o.is_entry()).map(|o| o.label()).collect();
    if entries.len() > 1 {
        return Err(format!("「{}」不能同時使用，它們吃的來源格式不同", entries.join("」與「")));
    }
    if !entries.is_empty() && ops.len() > 1 {
        // 入口轉換之後接別的步驟是合理的，只要它自己排在最前面即可，這裡不擋。
    }

    let terminals: Vec<&str> = ops.iter().filter(|o| o.is_terminal()).map(|o| o.label()).collect();
    if terminals.len() > 1 {
        return Err(format!("「{}」不能同時使用，它們都會產生非 PDF 的輸出", terminals.join("」與「")));
    }
    if !terminals.is_empty() && ops.iter().any(|o| matches!(o, Operation::Split { .. })) {
        return Err(format!("「分割」不能和「{}」一起用", terminals[0]));
    }

    let has_extract = ops.iter().any(|o| matches!(o, Operation::Extract { .. }));
    let has_remove = ops.iter().any(|o| matches!(o, Operation::Remove { .. }));
    if has_extract && has_remove {
        return Err("「擷取頁面」與「移除頁面」互相衝突，請只選一個".into());
    }

    // 用型別本身判斷重複，不能拿 order() 當識別——不同功能可以共用同一個順序，
    // 例如入口轉換的兩個工具。Discriminant 沒有排序，所以逐一比對而不是 sort+dedup
    // （dedup 只會移除相鄰的重複，不排序的話根本抓不到）。
    let mut seen: Vec<std::mem::Discriminant<Operation>> = Vec::with_capacity(ops.len());
    for op in ops {
        let d = std::mem::discriminant(op);
        if seen.contains(&d) {
            return Err(format!("「{}」不能重複勾選", op.label()));
        }
        seen.push(d);
    }
    Ok(())
}

#[derive(serde::Serialize, Clone)]
struct JobResult {
    id: String,
    /// 建議的輸出檔名，已含副檔名。使用者仍可在儲存對話框改。
    name: String,
    orig_bytes: u64,
    out_bytes: u64,
    error: Option<String>,
}

#[derive(serde::Serialize, Clone)]
struct FileInfo {
    path: String,
    name: String,
    bytes: u64,
    pages: Option<usize>,
    error: Option<String>,
}

#[derive(serde::Serialize)]
struct Engines {
    /// 隨程式打包，一定有；為 None 代表安裝檔不完整。
    ghostscript: Option<String>,
    /// 需使用者自行安裝，沒有屬正常情況。
    libreoffice: Option<String>,
    /// 獨立的擴充包，沒有屬正常情況。
    pyext: Option<String>,
    /// 隨程式打包，負責加解密。
    qpdf: Option<String>,
}

#[tauri::command]
fn engine_status(state: State<'_, AppState>) -> Engines {
    Engines {
        ghostscript: state.gs.as_ref().and_then(|p| gs::version(p).ok()),
        libreoffice: state.office.as_ref().and_then(|p| office::version(p).ok()),
        pyext: state.pyext.as_ref().and_then(|p| pyext::version(p).ok()),
        qpdf: state.qpdf.as_ref().and_then(|p| crypt::version(p).ok()),
    }
}

#[derive(serde::Serialize)]
struct FontChoice {
    label: String,
    path: String,
    index: u32,
}

/// 系統上可用的字型。浮水印與頁碼要內嵌字型，中文尤其挑不得。
///
/// 候選清單依平台而異（見 `platform::available_fonts`）。
#[tauri::command]
fn list_fonts() -> Vec<FontChoice> {
    platform::available_fonts()
        .into_iter()
        .map(|f| FontChoice {
            label: f.label,
            path: f.path.to_string_lossy().to_string(),
            index: f.index,
        })
        .collect()
}

/// 讀取檔案基本資訊。頁數讓 UI 能顯示並驗證頁碼範圍。
///
/// 圖片沒有頁數的概念，`pages` 留空即可——不是錯誤，UI 不該把它排除掉。
#[tauri::command]
async fn inspect(paths: Vec<String>) -> Vec<FileInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|raw| {
                let path = PathBuf::from(&raw);
                let name = file_name(&path, &raw);
                let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                if !name.to_lowercase().ends_with(".pdf") {
                    return FileInfo { path: raw, name, bytes, pages: None, error: None };
                }
                match pages::count(&path) {
                    Ok(n) => FileInfo { path: raw, name, bytes, pages: Some(n), error: None },
                    Err(e) => FileInfo { path: raw, name, bytes, pages: None, error: Some(e) },
                }
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// 產生第一頁的縮圖，讓檔案清單看得到內容而不只是檔名。
///
/// 刻意在 Rust 這側算圖再把 JPEG 位元組交出去——這樣整份 PDF 的內容仍然只存在於
/// Rust，前端拿到的只有一張小圖。若改用前端的 MuPDF 算縮圖會方便得多，
/// 但那等於為了視覺效果把每個工具的資料路徑都打開，不划算。
///
/// 36 dpi 就夠了：實測成本幾乎全在 Ghostscript 的行程啟動（約 180 ms），
/// 提高解析度並不會變慢多少，但檔案會變大。
#[tauri::command]
async fn thumbnail(state: State<'_, AppState>, path: String) -> Result<tauri::ipc::Response, String> {
    let gs = state.gs.clone().ok_or("找不到算圖引擎")?;
    let root = state.eph_root.clone();

    let bytes = tauri::async_runtime::spawn_blocking(move || {
        let eph = Ephemeral::new(&root).map_err(|e| format!("無法建立暫存檔：{e}"))?;
        gs::render_page(&gs, Path::new(&path), 1, 36, gs::ImageFormat::Jpeg, eph.path())?;
        // eph 在這個區塊結束時抹除，縮圖只以位元組形式離開。
        std::fs::read(eph.path()).map_err(|e| format!("讀不到縮圖：{e}"))
    })
    .await
    .map_err(|e| format!("工作執行緒異常：{e}"))??;

    Ok(tauri::ipc::Response::new(bytes))
}

/// 把檔案內容交給前端。
///
/// **只有「轉成 Markdown」會用到。** 其他工具的檔案內容都只存在於 Rust 這側，
/// 前端拿得到的僅有路徑。這裡開這個口是因為 MuPDF 只有 WASM 版可用，
/// 而它需要拿到位元組——理由見 `docs/edit-pdf-analysis.md` 第 8.4 節。
///
/// 回傳 `Response` 而不是 `Vec<u8>`：後者會被序列化成 JSON 數字陣列，
/// 一份 30 MB 的 PDF 會膨脹成上百 MB 的字串。
#[tauri::command]
fn read_file_bytes(path: String) -> Result<tauri::ipc::Response, String> {
    let data = std::fs::read(&path).map_err(|e| format!("無法讀取檔案：{e}"))?;
    Ok(tauri::ipc::Response::new(data))
}

/// 收下註記編輯器存出的 PDF，寫進暫存區並納入既有的儲存流程。
///
/// 用 base64 而不是 `Vec<u8>`：後者會被序列化成 JSON 數字陣列，一份 10 MB 的 PDF
/// 會膨脹成三四十 MB 的字串。base64 只多三分之一。
#[tauri::command]
fn save_edited(
    state: State<'_, AppState>,
    name: String,
    data_base64: String,
) -> Result<JobResult, String> {
    if data_base64.len() > 500 * 1024 * 1024 {
        return Err("檔案過大，超出編輯器儲存限制".into());
    }
    use base64::Engine as _;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(data_base64.as_bytes())
        .map_err(|e| format!("資料損毀：{e}"))?;
    if !bytes.starts_with(b"%PDF-") {
        return Err("產出的不是有效的 PDF".into());
    }

    let eph = Ephemeral::new(&state.eph_root).map_err(|e| format!("無法建立暫存檔：{e}"))?;
    std::fs::write(eph.path(), &bytes).map_err(|e| format!("無法寫出檔案：{e}"))?;

    let id = file_name(eph.path(), "");
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&name).to_string();
    let out_bytes = bytes.len() as u64;
    state.jobs.lock().unwrap().insert(id.clone(), eph);

    Ok(JobResult {
        id,
        name: format!("{stem}_annotated.pdf"),
        orig_bytes: 0,
        out_bytes,
        error: None,
    })
}

/// 把前端抽出來的結構化文字寫成 Markdown 暫存檔。
#[tauri::command]
fn markdown_from_lines(
    state: State<'_, AppState>,
    name: String,
    lines: Vec<markdown::TextLine>,
) -> Result<JobResult, String> {
    let content = markdown::render_structured(&lines);
    if content.trim().is_empty() {
        return Err("這份 PDF 沒有可抽取的文字，可能是掃描件——請先做 OCR".into());
    }
    let eph = Ephemeral::new(&state.eph_root).map_err(|e| format!("無法建立暫存檔：{e}"))?;
    std::fs::write(eph.path(), &content).map_err(|e| format!("無法寫出檔案：{e}"))?;

    let id = file_name(eph.path(), "");
    let out_bytes = content.len() as u64;
    let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(&name).to_string();
    state.jobs.lock().unwrap().insert(id.clone(), eph);

    Ok(JobResult {
        id,
        name: format!("{stem}.md"),
        orig_bytes: 0,
        out_bytes,
        error: None,
    })
}

/// 串接過程中的一份檔案。
///
/// `keep` 持有中間產物的 Ephemeral——只要它還在這個 Vec 裡就不會被抹除，
/// 一旦下一階段算完、整個 Vec 被替換掉，上一階段的中間檔就自動消失。
struct Item {
    path: PathBuf,
    /// 累積的檔名字尾，例如 `_compressed_watermarked`
    suffix: String,
    keep: Option<Ephemeral>,
}

#[tauri::command]
async fn run_pipeline(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    ops: Vec<Operation>,
) -> Result<Vec<JobResult>, String> {
    if paths.is_empty() {
        return Err("請先選擇檔案".into());
    }
    validate_pipeline(&ops)?;

    let mut ops = ops;
    ops.sort_by_key(Operation::order);

    if ops.iter().any(Operation::needs_gs) && state.gs.is_none() {
        return Err("找不到 Ghostscript 引擎，此功能無法使用".into());
    }
    if ops.iter().any(Operation::needs_office) && state.office.is_none() {
        return Err("找不到 LibreOffice。Office 轉檔需要先安裝它，安裝後請重新啟動本程式".into());
    }

    if ops.iter().any(Operation::needs_qpdf) && state.qpdf.is_none() {
        return Err("找不到加密引擎（qpdf），安裝檔可能不完整".into());
    }
    if ops.iter().any(Operation::needs_pyext) && state.pyext.is_none() {
        return Err(
            "PDF 轉 Word 需要另外安裝擴充包（約 100 MB）。安裝後請重新啟動本程式".into(),
        );
    }

    let root = state.eph_root.clone();
    let gs = state.gs.clone();
    let office = state.office.clone();
    let pyext = state.pyext.clone();
    let qpdf = state.qpdf.clone();
    let source_name = file_name(&PathBuf::from(&paths[0]), &paths[0]);
    let orig_bytes: u64 = paths
        .iter()
        .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
        .sum();

    CANCEL_REQUESTED.store(false, Ordering::SeqCst);

    let mut items: Vec<Item> = paths
        .into_iter()
        .map(|p| Item { path: PathBuf::from(p), suffix: String::new(), keep: None })
        .collect();
    let steps = ops.len();
    let mut extension = "pdf";

    for (step, op) in ops.iter().enumerate() {
        if is_cancelled() {
            return Err("操作已取消".into());
        }
        extension = op.extension();

        // 合併與圖片轉 PDF 是「多進一出」，整批送進去跑一次；其餘每份各跑一次。
        let batches: Vec<Vec<&Item>> = if op.merges() {
            vec![items.iter().collect()]
        } else {
            items.iter().map(|i| vec![i]).collect()
        };
        let batch_total = batches.len();

        let mut next: Vec<Item> = Vec::new();
        for (batch_idx, batch) in batches.into_iter().enumerate() {
            if is_cancelled() {
                return Err("操作已取消".into());
            }
            let _ = app.emit(
                "op:progress",
                serde_json::json!({
                    "index": step,
                    "total": steps,
                    "name": op.label(),
                    "fileIndex": batch_idx,
                    "fileTotal": batch_total,
                }),
            );
            let batch_paths: Vec<String> =
                batch.iter().map(|i| i.path.to_string_lossy().to_string()).collect();
            let carried = batch[0].suffix.clone();

            let (op2, gs, office, pyext, qpdf, root) = (
                op.clone(),
                gs.clone(),
                office.clone(),
                pyext.clone(),
                qpdf.clone(),
                root.clone(),
            );
            let outcome = tauri::async_runtime::spawn_blocking(move || {
                // 失敗時這一階段已建立的 Ephemeral 全部在此被 Drop 抹除。
                execute(
                    &op2,
                    &batch_paths,
                    Tools {
                        gs: gs.as_deref(),
                        office: office.as_deref(),
                        pyext: pyext.as_deref(),
                        qpdf: qpdf.as_deref(),
                    },
                    &root,
                )
            })
            .await
            .map_err(|e| format!("工作執行緒異常：{e}"))?;

            match outcome {
                Ok(outputs) => {
                    for (eph, suffix) in outputs {
                        next.push(Item {
                            path: eph.path().to_path_buf(),
                            suffix: format!("{carried}{suffix}"),
                            keep: Some(eph),
                        });
                    }
                }
                // 中途失敗就整批中止：後面的步驟建立在這一步的產出上，硬做下去只會
                // 產生一堆半成品，讓使用者更難判斷哪裡出錯。
                Err(error) => {
                    return Err(if steps > 1 {
                        format!("在「{}」這一步失敗：{error}", op.label())
                    } else {
                        error
                    })
                }
            }
        }
        // 舊的 items 在這裡被丟棄，上一階段的中間檔隨之抹除。
        items = next;
    }

    let stem = source_name
        .rsplit_once('.')
        .map(|(s, _)| s)
        .unwrap_or(&source_name)
        .to_string();

    let mut results = Vec::with_capacity(items.len());
    for (nth, item) in items.into_iter().enumerate() {
        let Some(eph) = item.keep else { continue };
        let id = file_name(eph.path(), "");
        let out_bytes = std::fs::metadata(eph.path()).map(|m| m.len()).unwrap_or(0);
        state.jobs.lock().unwrap().insert(id.clone(), eph);
        results.push(JobResult {
            id,
            name: format!("{stem}{}.{extension}", item.suffix),
            // 多個輸出時只有第一筆掛上來源大小，否則清單加總會膨脹成好幾倍。
            orig_bytes: if nth == 0 { orig_bytes } else { 0 },
            out_bytes,
            error: None,
        });
    }
    Ok(results)
}

/// 單一操作的捷徑，給還沒改用串接的呼叫端（目前是 Markdown 的備援路徑）。
#[tauri::command]
async fn run_op(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    op: Operation,
) -> Result<Vec<JobResult>, String> {
    run_pipeline(app, state, paths, vec![op]).await
}

/// 一次操作的產出：暫存檔，以及要接在來源檔名後面的字尾。
type Outputs = Vec<(Ephemeral, String)>;

/// 外部引擎的路徑。兩個都可能不存在，由各分支自行要求。
#[derive(Clone, Copy)]
struct Tools<'a> {
    gs: Option<&'a Path>,
    office: Option<&'a Path>,
    pyext: Option<&'a Path>,
    qpdf: Option<&'a Path>,
}

/// 實際執行一次操作。跑在 blocking pool 上。
///
/// 回傳多個輸出而非單一檔案，是因為分割與轉圖天生就是一進多出。
/// 中途失敗時已建立的 Ephemeral 會隨著 Vec 一起 Drop，全部抹除。
fn execute(op: &Operation, batch: &[String], tools: Tools, root: &Path) -> Result<Outputs, String> {
    let src = PathBuf::from(&batch[0]);
    let gs = tools.gs;
    let one = |suffix: &str| -> Result<(Ephemeral, String), String> {
        Ephemeral::new(root)
            .map(|e| (e, suffix.to_string()))
            .map_err(|e| format!("無法建立暫存檔：{e}"))
    };

    match op {
        Operation::Compress { level } => {
            let gs = gs.ok_or("找不到壓縮引擎（Ghostscript）")?;
            let out = one("_compressed")?;
            gs::compress(gs, &src, out.0.path(), *level)?;
            Ok(vec![out])
        }
        Operation::Merge => {
            let owned: Vec<PathBuf> = batch.iter().map(PathBuf::from).collect();
            let refs: Vec<&Path> = owned.iter().map(|p| p.as_path()).collect();
            let out = one("_merged")?;
            pages::merge(&refs, out.0.path())?;
            Ok(vec![out])
        }
        Operation::Extract { pages: keep } => {
            let out = one("_extracted")?;
            pages::select(&src, keep, out.0.path())?;
            Ok(vec![out])
        }
        Operation::Remove { pages: drop } => {
            let total = pages::count(&src)?;
            let keep: Vec<u32> = (1..=total as u32).filter(|n| !drop.contains(n)).collect();
            if keep.is_empty() {
                return Err("不能移除所有頁面".into());
            }
            let out = one("_removed")?;
            pages::select(&src, &keep, out.0.path())?;
            Ok(vec![out])
        }
        Operation::Rotate { pages: targets, degrees } => {
            let targets = expand(targets, &src)?;
            let out = one("_rotated")?;
            pages::rotate(&src, &targets, *degrees, out.0.path())?;
            Ok(vec![out])
        }
        Operation::Crop { pages: targets, margins } => {
            let targets = expand(targets, &src)?;
            let out = one("_cropped")?;
            pages::crop(&src, *margins, &targets, out.0.path())?;
            Ok(vec![out])
        }
        Operation::Split { chunk } => {
            if *chunk == 0 {
                return Err("每份頁數至少要 1".into());
            }
            let plan = pages::split_plan(pages::count(&src)?, *chunk);
            if plan.len() < 2 {
                return Err("這份文件的頁數不足以依此設定分割".into());
            }
            let mut outs: Outputs = Vec::with_capacity(plan.len());
            for part in &plan {
                let (first, last) = (part[0], part[part.len() - 1]);
                let suffix = if first == last {
                    format!("_p{first}")
                } else {
                    format!("_p{first}-{last}")
                };
                outs.push(one(&suffix)?);
            }
            let dests: Vec<&Path> = outs.iter().map(|(e, _)| e.path()).collect();
            pages::split(&src, &plan, &dests)?;
            Ok(outs)
        }
        Operation::ToImage { dpi, format } => {
            let gs = gs.ok_or("找不到轉檔引擎（Ghostscript）")?;
            let total = pages::count(&src)?;
            let mut outs: Outputs = Vec::with_capacity(total);
            for page in 1..=total as u32 {
                let out = one(&format!("_p{page:03}"))?;
                gs::render_page(gs, &src, page, *dpi, *format, out.0.path())?;
                outs.push(out);
            }
            Ok(outs)
        }
        Operation::ImagesToPdf { margin } => {
            let owned: Vec<PathBuf> = batch.iter().map(PathBuf::from).collect();
            let refs: Vec<&Path> = owned.iter().map(|p| p.as_path()).collect();
            let out = one("_images")?;
            images::images_to_pdf(&refs, *margin, out.0.path())?;
            Ok(vec![out])
        }
        Operation::Repair => {
            let gs = gs.ok_or("找不到修復引擎（Ghostscript）")?;
            let out = one("_repaired")?;
            gs::repair(gs, &src, out.0.path())?;
            Ok(vec![out])
        }
        Operation::Watermark { text, style, angle, pages: targets } => {
            let out = one("_watermarked")?;
            annotate::watermark(&src, out.0.path(), text, style, *angle, targets)?;
            Ok(vec![out])
        }
        Operation::PageNumbers { template, style, corner, start_at, margin, pages: targets } => {
            let out = one("_numbered")?;
            annotate::page_numbers(
                &src,
                out.0.path(),
                template,
                style,
                *corner,
                *start_at,
                *margin,
                targets,
            )?;
            Ok(vec![out])
        }
        Operation::Stamp { image, place, pages: targets } => {
            let out = one("_stamped")?;
            annotate::stamp_image(&src, out.0.path(), Path::new(image), place, targets)?;
            Ok(vec![out])
        }
        Operation::ToMarkdown => {
            let gs = gs.ok_or("找不到文字抽取引擎（Ghostscript）")?;
            let out = one("")?;
            markdown::to_markdown(gs, &src, out.0.path())?;
            Ok(vec![out])
        }
        Operation::Encrypt { user, owner } => {
            let qpdf = tools.qpdf.ok_or("找不到加密引擎（qpdf）")?;
            let out = one("_protected")?;
            crypt::encrypt(qpdf, &src, out.0.path(), user, owner)?;
            Ok(vec![out])
        }
        Operation::Decrypt { password } => {
            let qpdf = tools.qpdf.ok_or("找不到加密引擎（qpdf）")?;
            let out = one("_unlocked")?;
            crypt::decrypt(qpdf, &src, out.0.path(), password)?;
            Ok(vec![out])
        }
        Operation::PdfToWord { fallback_font } => {
            let py = tools.pyext.ok_or("需要安裝擴充包才能使用 PDF 轉 Word")?;
            let out = one("")?;
            pyext::pdf_to_docx(py, &src, out.0.path(), fallback_font)?;
            Ok(vec![out])
        }
        Operation::OfficeToPdf => {
            let soffice = tools.office.ok_or("找不到 LibreOffice")?;
            let out = one("")?;
            // LibreOffice 只能指定輸出資料夾不能指定檔名，所以先讓它寫進暫存根目錄
            // 底下的工作區——這樣萬一當掉，啟動掃除也涵蓋得到。
            let workdir = root.join(format!("office-{}", out.0.path().file_name().unwrap().to_string_lossy()));
            office::to_pdf(soffice, &src, &workdir, out.0.path())?;
            Ok(vec![out])
        }
    }
}

/// 頁碼清單為空時代表整份文件。
fn expand(targets: &[u32], src: &Path) -> Result<Vec<u32>, String> {
    if targets.is_empty() {
        Ok((1..=pages::count(src)? as u32).collect())
    } else {
        Ok(targets.to_vec())
    }
}

fn file_name(path: &Path, fallback: &str) -> String {
    path.file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| fallback.to_string())
}

/// 把結果搬到使用者選定的位置，然後讓暫存檔消失。
#[tauri::command]
fn save_job(state: State<'_, AppState>, id: String, dest: String) -> Result<(), String> {
    let eph = state
        .jobs
        .lock()
        .unwrap()
        .remove(&id)
        .ok_or("這個項目已經不存在了")?;
    let src = eph.path().to_path_buf();
    let dest = PathBuf::from(dest);

    // 同磁碟時 rename 是原子操作，而且不會留下第二份內容。
    if std::fs::rename(&src, &dest).is_ok() {
        let _ = eph.release();
        return Ok(());
    }
    // 跨磁碟只能複製，複製完立刻抹除來源。
    std::fs::copy(&src, &dest).map_err(|e| format!("儲存失敗：{e}"))?;
    eph.destroy();
    Ok(())
}

/// 清空所有未儲存的結果。UI 的「全部清除」與關閉視窗時都會呼叫。
#[tauri::command]
fn discard_all(state: State<'_, AppState>) {
    state.jobs.lock().unwrap().clear();
    ephemeral::sweep(&state.eph_root);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 命令層自己算出來的東西只有兩處：移除頁面的補集、旋轉留空代表全部。
    /// pages.rs 的測試涵蓋不到這兩段，所以放在這裡。
    fn corpus(name: &str) -> Option<PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .join("bench/corpus")
            .join(name);
        p.exists().then_some(p)
    }

    /// 每個測試用自己的暫存根目錄，Ephemeral 才不會互相干擾。
    fn eph_root() -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "localpdf-cmd-{:x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    fn run(op: &Operation, src: &Path, root: &Path) -> Result<Outputs, String> {
        let gs = gs::locate(None);
        let office = office::locate();
        let py = pyext::locate(None);
        execute(
            op,
            &[src.to_string_lossy().into()],
            Tools { gs: gs.as_deref(), office: office.as_deref(), pyext: py.as_deref(), qpdf: crypt::locate(None).as_deref() },
            root,
        )
    }

    #[test]
    fn remove_keeps_the_complement() {
        let Some(src) = corpus("c1_text.pdf") else {
            eprintln!("跳過：語料不存在，請先跑 bench/make_corpus.py");
            return;
        };
        let total = pages::count(&src).unwrap();
        let root = eph_root();
        let out = run(&Operation::Remove { pages: vec![1, 2, 3] }, &src, &root).unwrap();

        assert_eq!(out.len(), 1);
        assert_eq!(pages::count(out[0].0.path()).unwrap(), total - 3);
    }

    #[test]
    fn removing_everything_is_rejected() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let total = pages::count(&src).unwrap() as u32;
        let op = Operation::Remove { pages: (1..=total).collect() };
        let err = run(&op, &src, &eph_root()).unwrap_err();
        assert!(err.contains("不能移除所有頁面"), "訊息應說明原因：{err}");
    }

    #[test]
    fn rotate_with_no_pages_means_all_pages() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let root = eph_root();
        let out = run(&Operation::Rotate { pages: vec![], degrees: 90 }, &src, &root).unwrap();

        let doc = lopdf::Document::load(out[0].0.path()).unwrap();
        let rotated = doc
            .get_pages()
            .values()
            .filter(|id| {
                doc.get_object(**id)
                    .and_then(|o| o.as_dict())
                    .ok()
                    .and_then(|d| d.get(b"Rotate").ok()?.as_i64().ok())
                    == Some(90)
            })
            .count();
        assert_eq!(rotated, doc.get_pages().len(), "留空應該旋轉每一頁");
    }

    #[test]
    fn split_produces_one_file_per_chunk() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let total = pages::count(&src).unwrap();
        let root = eph_root();
        let out = run(&Operation::Split { chunk: 10 }, &src, &root).unwrap();

        assert_eq!(out.len(), total.div_ceil(10));
        assert_eq!(pages::count(out[0].0.path()).unwrap(), 10);
        assert_eq!(out[0].1, "_p1-10", "檔名字尾要標出頁碼範圍");
    }

    #[test]
    fn split_refuses_when_it_would_produce_one_file() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let op = Operation::Split { chunk: 9999 };
        let err = run(&op, &src, &eph_root()).unwrap_err();
        assert!(err.contains("頁數不足"), "應說明為何不能分割：{err}");
    }

    #[test]
    fn to_jpeg_produces_one_image_per_page() {
        let Some(src) = corpus("c3_mixed.pdf") else { return };
        if gs::locate(None).is_none() {
            eprintln!("跳過：找不到 Ghostscript");
            return;
        }
        let total = pages::count(&src).unwrap();
        let root = eph_root();
        let op = Operation::ToImage { dpi: 72, format: gs::ImageFormat::Jpeg };
        let out = run(&op, &src, &root).unwrap();

        assert_eq!(out.len(), total);
        assert_eq!(out[0].1, "_p001");
        // JPEG 的檔頭是 FF D8 FF——確認真的產出圖片而不是空檔或 PDF。
        let head = std::fs::read(out[0].0.path()).unwrap();
        assert_eq!(&head[..3], &[0xFF, 0xD8, 0xFF], "輸出應該是 JPEG");
    }

    #[test]
    fn to_image_rejects_absurd_dpi() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        if gs::locate(None).is_none() {
            return;
        }
        let op = Operation::ToImage { dpi: 5000, format: gs::ImageFormat::Jpeg };
        assert!(run(&op, &src, &eph_root()).is_err());
    }

    /// 端到端：真的從 PDF 抽出中文並整理成 Markdown。
    ///
    /// markdown.rs 的單元測試用的是固定樣本，證明不了 Ghostscript 那一段；
    /// 中文編碼是這條路徑最容易壞的地方，所以這裡要用真檔案跑一次。
    // ---- 串接 ----

    #[test]
    fn pipeline_runs_in_canonical_order_not_click_order() {
        let mut ops = vec![
            Operation::Compress { level: gs::Level::Recommended },
            Operation::Extract { pages: vec![1] },
            Operation::Repair,
        ];
        ops.sort_by_key(Operation::order);
        let labels: Vec<&str> = ops.iter().map(|o| o.label()).collect();
        // 修復要在動結構之前，壓縮一定要最後——先壓再改只會抵銷壓縮效果
        assert_eq!(labels, vec!["修復", "擷取頁面", "壓縮"]);
    }

    #[test]
    fn pipeline_rejects_conflicting_combinations() {
        let cases: Vec<(Vec<Operation>, &str)> = vec![
            (vec![], "至少勾選"),
            (
                vec![
                    Operation::Extract { pages: vec![1] },
                    Operation::Remove { pages: vec![2] },
                ],
                "衝突",
            ),
            (
                vec![
                    Operation::ToMarkdown,
                    Operation::ToImage { dpi: 150, format: gs::ImageFormat::Jpeg },
                ],
                "非 PDF",
            ),
            (
                vec![Operation::ToMarkdown, Operation::Split { chunk: 2 }],
                "分割",
            ),
            (
                vec![Operation::OfficeToPdf, Operation::ImagesToPdf { margin: 0.0 }],
                "來源格式不同",
            ),
            (
                vec![
                    Operation::Rotate { pages: vec![], degrees: 90 },
                    Operation::Rotate { pages: vec![], degrees: 180 },
                ],
                "重複勾選",
            ),
        ];
        for (ops, expect) in cases {
            let err = validate_pipeline(&ops).unwrap_err();
            assert!(err.contains(expect), "訊息應包含「{expect}」，實得：{err}");
        }
    }

    #[test]
    fn pipeline_accepts_a_sensible_chain() {
        let ops = vec![
            Operation::Compress { level: gs::Level::Recommended },
            Operation::Rotate { pages: vec![], degrees: 90 },
            Operation::Split { chunk: 5 },
        ];
        assert!(validate_pipeline(&ops).is_ok());
    }

    /// 串接的每一步都要真的作用到上一步的產出。
    #[test]
    fn chained_steps_all_take_effect() {
        let Some(src) = corpus("c2_scan.pdf") else {
            eprintln!("跳過：語料不存在");
            return;
        };
        if gs::locate(None).is_none() {
            return;
        }
        let root = eph_root();
        let tools = Tools { gs: None, office: None, pyext: None, qpdf: None };

        // 先擷取 4 頁
        let picked = execute(
            &Operation::Extract { pages: vec![1, 2, 3, 4] },
            &[src.to_string_lossy().into()],
            tools,
            &root,
        )
        .unwrap();
        assert_eq!(pages::count(picked[0].0.path()).unwrap(), 4);

        // 再從這 4 頁分成每份 2 頁
        let split = execute(
            &Operation::Split { chunk: 2 },
            &[picked[0].0.path().to_string_lossy().into()],
            tools,
            &root,
        )
        .unwrap();
        assert_eq!(split.len(), 2, "4 頁切成每份 2 頁應該得到 2 個檔案");
        assert_eq!(pages::count(split[0].0.path()).unwrap(), 2);
    }

    /// 圖章要能蓋在每一頁，且不改變頁數。
    #[test]
    fn stamp_applies_to_all_pages() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let root = eph_root();
        let sign = root.join("seal.png");
        {
            let (w, h) = (30u32, 30u32);
            let file = std::fs::File::create(&sign).unwrap();
            let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .unwrap()
                .write_image_data(&vec![200u8; (w * h * 4) as usize])
                .unwrap();
        }
        let op = Operation::Stamp {
            image: sign.to_string_lossy().into(),
            place: annotate::Placement {
                corner: annotate::Corner::BottomRight,
                margin: 20.0,
                width: 80.0,
                opacity: 0.9,
            },
            pages: vec![],
        };
        let out = run(&op, &src, &root).unwrap();
        assert_eq!(
            pages::count(out[0].0.path()).unwrap(),
            pages::count(&src).unwrap(),
            "蓋章不該改變頁數"
        );
    }

    #[test]
    fn markdown_extracts_real_chinese_text() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        if gs::locate(None).is_none() {
            eprintln!("跳過：找不到 Ghostscript");
            return;
        }
        let root = eph_root();
        let out = run(&Operation::ToMarkdown, &src, &root).unwrap();
        let text = std::fs::read_to_string(out[0].0.path()).unwrap();

        assert!(text.contains("壓縮測試"), "中文應該正確抽出，實得開頭：{:?}", &text[..80.min(text.len())]);
        assert!(text.contains('#'), "應該有偵測到標題");
    }

    #[test]
    fn to_png_produces_png_output() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        if gs::locate(None).is_none() {
            return;
        }
        let root = eph_root();
        let op = Operation::ToImage { dpi: 72, format: gs::ImageFormat::Png };
        let out = run(&op, &src, &root).unwrap();

        // PNG 檔頭：89 50 4E 47
        let head = std::fs::read(out[0].0.path()).unwrap();
        assert_eq!(&head[..4], &[0x89, 0x50, 0x4E, 0x47], "輸出應該是 PNG");
    }

    #[test]
    fn repair_rebuilds_a_truncated_file() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        if gs::locate(None).is_none() {
            return;
        }
        let root = eph_root();
        // 砍掉尾端 5%，交叉參照表會壞掉——這是最常見的損毀樣態。
        let data = std::fs::read(&src).unwrap();
        let broken = root.join("broken.pdf");
        std::fs::write(&broken, &data[..data.len() * 95 / 100]).unwrap();
        assert!(pages::count(&broken).is_err(), "前提：這份檔案應該是讀不開的");

        let out = run(&Operation::Repair, &broken, &root).unwrap();
        let recovered = pages::count(out[0].0.path()).unwrap();
        assert!(recovered > 0, "修復後應該至少救回一頁");
    }

    /// 一進多出時，來源大小只掛在第一筆結果上——否則清單加總會膨脹成好幾倍。
    #[test]
    fn multi_output_counts_source_size_once() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let root = eph_root();
        let outs = run(&Operation::Split { chunk: 10 }, &src, &root).unwrap();
        let orig = std::fs::metadata(&src).unwrap().len();

        let totals: Vec<u64> = (0..outs.len())
            .map(|nth| if nth == 0 { orig } else { 0 })
            .collect();
        assert_eq!(totals.iter().sum::<u64>(), orig);
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let data_dir = app.path().app_local_data_dir()?;
            let eph_root = ephemeral::root(&data_dir);
            std::fs::create_dir_all(&eph_root)?;
            // 開機第一件事：清掉上次當掉留下的殘骸。
            ephemeral::sweep(&eph_root);

            let resource_dir = app.path().resource_dir().ok();
            let gs = gs::locate(resource_dir.as_deref());

            app.manage(AppState {
                gs,
                office: office::locate(),
                pyext: pyext::locate(resource_dir.as_deref()),
                qpdf: crypt::locate(resource_dir.as_deref()),
                eph_root,
                jobs: Mutex::new(HashMap::new()),
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if matches!(event, tauri::WindowEvent::CloseRequested { .. }) {
                let state = window.state::<AppState>();
                state.jobs.lock().unwrap().clear();
                ephemeral::sweep(&state.eph_root);
            }
        })
        .invoke_handler(tauri::generate_handler![
            engine_status,
            list_fonts,
            inspect,
            thumbnail,
            read_file_bytes,
            markdown_from_lines,
            save_edited,
            run_pipeline,
            run_op,
            cancel_pipeline,
            save_job,
            discard_all
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}



