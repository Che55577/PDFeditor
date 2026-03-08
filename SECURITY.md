# 安全政策（Security Policy）

## 支援版本

目前維護並接受安全漏洞回報的版本如下：

| 版本 | 支援狀態 |
| :--- | :--- |
| `0.1.x` (`main`) | 支援中 |

## 安全架構說明

LocalPDF 處理使用者可能高度敏感的本機 PDF 與 Office 文件，並將悪意構造的輸入檔（Malicious PDF / Office / ZIP）視為不信任輸入（Untrusted Input）。主要安全防線包含：

- **PostScript 沙箱**：呼叫 Ghostscript 時強制帶入 `-dSAFER`，封鎖檔案系統任意讀寫與外部指令執行。
- **密碼零暴露**：呼叫 `qpdf` 時透過 `@-` 由標準輸入（`stdin`）傳遞密碼與參數，並立即關閉 `stdin`，避免密碼殘留於作業系統行程清單（Process List）或暫存檔。
- **ZIP 炸彈防護**：修改 `.docx` 結構（`src-tauri/src/docxfont.rs`）時，限制單一項目展開上限（64 MB）與總展開大小上限（256 MB），超限時立即中止並回報錯誤。
- **受控子行程與逾時清理**：所有外部命令透過 `src-tauri/src/platform.rs` 的 `run_with_timeout` 執行，以背景執行緒排空 `stdout` / `stderr` 防止管線死結，並支援逾時與使用者主動取消。
- **暫存檔覆寫清理**：所有中間檔由 `src-tauri/src/ephemeral.rs` 管理，使用完畢後先以零覆寫（Zero-fill）並 `sync_all` 再刪除。
- **WebView CSP 限制**：透過 `src-tauri/tauri.conf.json` 實施嚴格的 Content-Security-Policy，禁止任何對外網路連線。

## 回報安全漏洞

若你發現任何潛在的安全問題（例如：命令注入、任意檔案讀寫、沙箱逃逸、密碼外洩或資源耗盡 DoS），**請勿直接在公開 Issue 中揭露利用細節**。

請透過以下管道私下回報：

1. 使用 GitHub 倉庫的 **Private Vulnerability Reporting**（位於 `Security` 頁籤 -> `Report a vulnerability`）。
2. 在報告中附上：
   - 漏洞類型與受影響的模組（例如 `src-tauri/src/gs.rs`）
   - 最小可重現步驟（PoC 或概念說明）
   - 潛在影響範圍與建議修復方向

維護者將於收到回報後儘速確認並進行修復。
