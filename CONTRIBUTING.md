# 貢獻指南（Contributing Guide）

感謝你對 **LocalPDF** 的關注與貢獻！本指南說明如何在本機建置、測試與提交變更。

## 開發環境需求

- **Node.js** 20 或以上（含 `npm`）
- **Rust** stable 工具鏈（`rustup` / `cargo`）
- **Python** 3.11 或以上（用於產生測試語料庫與 `pyext` 擴充包打包）
- **Linux 額外系統套件**（Ubuntu / Debian）：
  ```bash
  sudo apt-get update
  sudo apt-get install -y \
    libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf \
    ghostscript qpdf fonts-noto-cjk
  ```

## 快速啟動

```bash
# 1. 安裝前端相依套件（postinstall 會自動同步 MuPDF WASM 資產至 public/mupdf/）
npm ci

# 2. 下載或編譯外部二進位引擎（Ghostscript 10.07.1 與 qpdf 12.4.2，含 SHA-256 校驗）
npm run fetch:vendor

# 3. 啟動開發模式
npm run tauri dev
```

## 測試與基準驗證

提交 Pull Request 前，請確認以下檢查皆通過：

```bash
# 產生整合測試語料庫
pip install reportlab pillow
python bench/make_corpus.py

# 前端 TypeScript 型別檢查與打包
npm run build

# Rust 單元測試與整合測試
cargo test --manifest-path src-tauri/Cargo.toml
```

## 設計與安全原則

1. **零網路連線保證**：所有檔案處理必須在離線本機環境完成，禁止引入任何遙測、外部 CDN 或雲端 API。
2. **暫存檔生命週期管理**：所有落地的暫存檔必須透過 `src-tauri/src/ephemeral.rs`（`Ephemeral` 控制器與 `wipe` / `wipe_dir_all`）管理，確保作業完成、失敗或啟動掃除時皆先覆寫為零再刪除。
3. **子行程安全**：外部引擎（Ghostscript、qpdf、LibreOffice、Python）一律透過 `platform::run_with_timeout` / `run_with_timeout_stdin` 執行，禁止拼接 Shell 字串；密碼等敏感參數必須透過 `stdin` 傳遞（如 `qpdf @-`），不得出現在命令列參數中。
4. **授權合規**：本專案採用 **AGPL-3.0-or-later** 授權，新增第三方相依套件時請確認相容性並更新 `THIRD_PARTY_NOTICES.md`。

## 提交流程

1. Fork 本倉庫並從 `main` 建立功能分支（例如 `feat/your-feature` 或 `fix/issue-description`）。
2. 確保新增功能或修復附帶對應的自動化測試。
3. 提交 Pull Request 並說明變更動機、影響範圍與驗證方式。
