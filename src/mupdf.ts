/**
 * MuPDF (WebAssembly) 的載入器。
 *
 * 這是**唯一會把檔案內容讀進 WebView 的相依**。其他工具的檔案內容都只存在於
 * Rust 那側，前端拿得到的僅有路徑。會開這個口是因為 MuPDF 只有 WASM 版可用——
 * 原生的 mupdf-rs 綑綁 C 原始碼、需要 libclang，且 Windows/MSVC 建置被回報不穩定。
 * 完整理由見 docs/edit-pdf-analysis.md 第 8.4 節。
 */

export type MuPdf = typeof import("mupdf");

let cached: Promise<MuPdf> | null = null;

/**
 * 從 `/mupdf/` 這個固定路徑載入，刻意不讓 Vite 打包。
 *
 * MuPDF 用 `new URL("mupdf-wasm.wasm", import.meta.url)` 找自己的 WASM，
 * 一旦被打包，模組會變成 `/assets/index-xxxx.js`，WASM 就會被找到 `/assets/` 底下
 * ——那裡沒有東西。用變數當 specifier 可以讓 TypeScript 與 Vite 都不去解析它。
 *
 * WASM 有 10 MB，所以只在真的要用時才載入。
 */
const ENTRY = "/mupdf/mupdf.js";

/**
 * 用 `new Function` 包住動態 import，讓 Vite 完全看不到它。
 *
 * 這不是為了炫技。Vite 在開發模式會把所有動態 import 改寫成
 * `__vite__injectQuery(url, "import")`，於是請求變成 `/mupdf/mupdf.js?import`
 * ——而 Vite 明確不支援從 `public/` 匯入模組，那個 URL 會回 500。
 * `@vite-ignore` 註解在這個情況下擋不住改寫，實測過。
 *
 * 副作用：需要 `unsafe-eval`。目前 `tauri.conf.json` 的 CSP 是 null 所以沒問題，
 * 但如果之後要開 CSP，這裡要一併加上（WASM 本來也需要 `wasm-unsafe-eval`）。
 */
const dynamicImport = new Function("url", "return import(url)") as (u: string) => Promise<MuPdf>;

export function loadMuPdf(): Promise<MuPdf> {
  cached ??= dynamicImport(ENTRY);
  return cached;
}
