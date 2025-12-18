mod ephemeral;
mod gs;

use std::collections::HashMap;
use std::path::PathBuf;
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

#[derive(serde::Serialize, Clone)]
struct JobResult {
    id: String,
    name: String,
    orig_bytes: u64,
    out_bytes: u64,
    error: Option<String>,
}

#[tauri::command]
fn engine_status(state: State<'_, AppState>) -> Result<String, String> {
    let gs = state.gs.as_ref().ok_or("找不到壓縮引擎（Ghostscript）")?;
    gs::version(gs)
}

#[tauri::command]
async fn compress_files(
    app: AppHandle,
    state: State<'_, AppState>,
    paths: Vec<String>,
    level: Level,
) -> Result<Vec<JobResult>, String> {
    let gs = state.gs.clone().ok_or("找不到壓縮引擎（Ghostscript）")?;
    let root = state.eph_root.clone();
    let total = paths.len();
    let mut results = Vec::with_capacity(total);

    for (index, raw) in paths.into_iter().enumerate() {
        let src = PathBuf::from(&raw);
        let name = src
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| raw.clone());

        let _ = app.emit(
            "compress:progress",
            serde_json::json!({ "index": index, "total": total, "name": name }),
        );

        let orig_bytes = std::fs::metadata(&src).map(|m| m.len()).unwrap_or(0);
        let (gs, root) = (gs.clone(), root.clone());

        // 壓縮是 CPU/IO 密集的外部行程，丟到 blocking pool 以免卡住 UI。
        let outcome = tauri::async_runtime::spawn_blocking(move || {
            // 失敗時 eph 在此函式結束就被 Drop 抹除，不需要額外清理。
            let eph = Ephemeral::new(&root).map_err(|e| format!("無法建立暫存檔：{e}"))?;
            gs::compress(&gs, &src, eph.path(), level)?;
            let out_bytes = std::fs::metadata(eph.path()).map(|m| m.len()).unwrap_or(0);
            Ok::<_, String>((eph, out_bytes))
        })
        .await
        .map_err(|e| format!("工作執行緒異常：{e}"))?;

        results.push(match outcome {
            Ok((eph, out_bytes)) => {
                let id = eph
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_default();
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
            compress_files,
            save_job,
            discard_all
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
