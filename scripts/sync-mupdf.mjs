/**
 * 把 MuPDF 的執行期檔案從 node_modules 複製到 public/mupdf/。
 *
 * 為什麼不讓 Vite 直接打包 mupdf：它用 `new URL("mupdf-wasm.wasm", import.meta.url)`
 * 找自己的 WASM，打包後模組路徑會變成 /assets/index-xxxx.js，WASM 就會被找到
 * /assets/ 底下——那裡沒有東西。放在 public/ 底下路徑才是固定的。
 *
 * 為什麼不把檔案直接進版控：那是 10 MB 的建置產物，可以從 node_modules 重現，
 * 跟 vendor/ghostscript 一樣不該佔用版本庫。
 */
import { copyFileSync, existsSync, mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const from = join(root, "node_modules", "mupdf", "dist");
const to = join(root, "public", "mupdf");

const FILES = ["mupdf.js", "mupdf-wasm.js", "mupdf-wasm.wasm"];

if (!existsSync(from)) {
  console.error("找不到 node_modules/mupdf，請先執行 npm install");
  process.exit(1);
}

mkdirSync(to, { recursive: true });
for (const name of FILES) {
  copyFileSync(join(from, name), join(to, name));
}
console.log(`已同步 MuPDF 執行期檔案到 public/mupdf/（${FILES.length} 個）`);
