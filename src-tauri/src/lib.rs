mod ephemeral;
mod gs;
mod pages;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use ephemeral::Ephemeral;
use gs::Level;
use tauri::{AppHandle, Emitter, Manager, State};

struct AppState {
    /// 壓縮引擎路徑。找不到時為 None，UI 會顯示引擎缺失而不是讓每次操作都失敗。
    gs: Option<PathBuf>,
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
}

impl Operation {
    fn merges(&self) -> bool {
        matches!(self, Operation::Merge)
    }
}

#[derive(serde::Serialize, Clone)]
struct JobResult {
    id: String,
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

#[tauri::command]
fn engine_status(state: State<'_, AppState>) -> Result<String, String> {
    let gs = state.gs.as_ref().ok_or("找不到壓縮引擎（Ghostscript）")?;
    gs::version(gs)
}

/// 讀取檔案基本資訊。頁數讓 UI 能顯示並驗證頁碼範圍。
#[tauri::command]
async fn inspect(paths: Vec<String>) -> Vec<FileInfo> {
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .map(|raw| {
                let path = PathBuf::from(&raw);
                let name = file_name(&path, &raw);
                let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
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

#[tauri::command]
async fn run_op(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    op: Operation,
) -> Result<Vec<JobResult>, String> {
    if paths.is_empty() {
        return Err("請先選擇檔案".into());
    }
    if let Operation::Compress { .. } = op {
        if state.gs.is_none() {
            return Err("找不到壓縮引擎（Ghostscript）".into());
        }
    }
    let root = state.eph_root.clone();
    let gs = state.gs.clone();

    // 合併是唯一的「多進一出」，輸入清單整批送進去跑一次。
    let batches: Vec<Vec<String>> = if op.merges() {
        vec![paths]
    } else {
        paths.into_iter().map(|p| vec![p]).collect()
    };
    let total = batches.len();
    let mut results = Vec::with_capacity(total);

    for (index, batch) in batches.into_iter().enumerate() {
        let first = PathBuf::from(&batch[0]);
        let name = if op.merges() {
            format!("{} 等 {} 個檔案", file_name(&first, &batch[0]), batch.len())
        } else {
            file_name(&first, &batch[0])
        };

        let _ = app.emit(
            "op:progress",
            serde_json::json!({ "index": index, "total": total, "name": name }),
        );

        let orig_bytes = batch
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
            .sum();

        let (op, gs, root) = (op.clone(), gs.clone(), root.clone());
        let outcome = tauri::async_runtime::spawn_blocking(move || {
            // 失敗時 eph 在此函式結束就被 Drop 抹除，不需要額外清理。
            let eph = Ephemeral::new(&root).map_err(|e| format!("無法建立暫存檔：{e}"))?;
            execute(&op, &batch, gs.as_deref(), eph.path())?;
            let out_bytes = std::fs::metadata(eph.path()).map(|m| m.len()).unwrap_or(0);
            Ok::<_, String>((eph, out_bytes))
        })
        .await
        .map_err(|e| format!("工作執行緒異常：{e}"))?;

        results.push(match outcome {
            Ok((eph, out_bytes)) => {
                let id = file_name(eph.path(), "");
                state.jobs.lock().unwrap().insert(id.clone(), eph);
                JobResult { id, name, orig_bytes, out_bytes, error: None }
            }
            Err(error) => JobResult {
                id: String::new(),
                name,
                orig_bytes,
                out_bytes: 0,
                error: Some(error),
            },
        });
    }
    Ok(results)
}

/// 實際執行一次操作。跑在 blocking pool 上。
fn execute(op: &Operation, batch: &[String], gs: Option<&Path>, out: &Path) -> Result<(), String> {
    let src = PathBuf::from(&batch[0]);
    match op {
        Operation::Compress { level } => {
            let gs = gs.ok_or("找不到壓縮引擎（Ghostscript）")?;
            gs::compress(gs, &src, out, *level)
        }
        Operation::Merge => {
            let owned: Vec<PathBuf> = batch.iter().map(PathBuf::from).collect();
            let refs: Vec<&Path> = owned.iter().map(|p| p.as_path()).collect();
            pages::merge(&refs, out)
        }
        Operation::Extract { pages: keep } => pages::select(&src, keep, out),
        Operation::Remove { pages: drop } => {
            let total = pages::count(&src)?;
            let keep: Vec<u32> = (1..=total as u32).filter(|n| !drop.contains(n)).collect();
            if keep.is_empty() {
                return Err("不能移除所有頁面".into());
            }
            pages::select(&src, &keep, out)
        }
        Operation::Rotate { pages: targets, degrees } => {
            let targets = if targets.is_empty() {
                (1..=pages::count(&src)? as u32).collect()
            } else {
                targets.clone()
            };
            pages::rotate(&src, &targets, *degrees, out)
        }
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

    fn tmp(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("localpdf-cmd-{name}"))
    }

    #[test]
    fn remove_keeps_the_complement() {
        let Some(src) = corpus("c1_text.pdf") else {
            eprintln!("跳過：語料不存在，請先跑 bench/make_corpus.py");
            return;
        };
        let total = pages::count(&src).unwrap();
        let out = tmp("remove.pdf");
        let op = Operation::Remove { pages: vec![1, 2, 3] };

        execute(&op, &[src.to_string_lossy().into()], None, &out).unwrap();
        assert_eq!(pages::count(&out).unwrap(), total - 3);
        std::fs::remove_file(&out).ok();
    }

    #[test]
    fn removing_everything_is_rejected() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let total = pages::count(&src).unwrap() as u32;
        let op = Operation::Remove { pages: (1..=total).collect() };
        let err = execute(&op, &[src.to_string_lossy().into()], None, &tmp("x.pdf")).unwrap_err();
        assert!(err.contains("不能移除所有頁面"), "訊息應說明原因：{err}");
    }

    #[test]
    fn rotate_with_no_pages_means_all_pages() {
        let Some(src) = corpus("c1_text.pdf") else { return };
        let out = tmp("rotate-all.pdf");
        let op = Operation::Rotate { pages: vec![], degrees: 90 };

        execute(&op, &[src.to_string_lossy().into()], None, &out).unwrap();

        let doc = lopdf::Document::load(&out).unwrap();
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
        std::fs::remove_file(&out).ok();
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
            inspect,
            run_op,
            save_job,
            discard_all
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
