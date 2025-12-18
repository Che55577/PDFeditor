import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";

type Tool =
  | "compress"
  | "merge"
  | "extract"
  | "remove"
  | "rotate"
  | "crop"
  | "split"
  | "toimage"
  | "imagestopdf"
  | "repair"
  | "watermark"
  | "pagenumbers"
  | "officetopdf";
type Level = "extreme" | "recommended" | "low";

interface FileInfo {
  path: string;
  name: string;
  bytes: number;
  pages: number | null;
  error: string | null;
}

interface JobResult {
  id: string;
  name: string;
  orig_bytes: number;
  out_bytes: number;
  error: string | null;
}

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;

const dropEl = $<HTMLElement>("drop");
const listEl = $<HTMLUListElement>("list");
const statusEl = $<HTMLElement>("status");
const engineEl = $<HTMLElement>("engine");
const toolsEl = $<HTMLElement>("tools");
const runBtn = $<HTMLButtonElement>("run");
const saveAllBtn = $<HTMLButtonElement>("saveAll");
const clearBtn = $<HTMLButtonElement>("clear");

let tool: Tool = "compress";
/** 已載入、待處理的檔案。只留路徑與中繼資料，內容不進前端記憶體。 */
let files: FileInfo[] = [];
/** 已完成、尚未存檔的結果。每一筆在後端都對應一個暫存檔。 */
let results: JobResult[] = [];
let busy = false;

const LABEL: Record<Tool, string> = {
  compress: "壓縮",
  merge: "合併",
  extract: "擷取頁面",
  remove: "移除頁面",
  rotate: "旋轉",
  crop: "裁切",
  split: "分割",
  toimage: "轉成圖片",
  imagestopdf: "轉成 PDF",
  repair: "修復",
  watermark: "加浮水印",
  pagenumbers: "加頁碼",
  officetopdf: "轉成 PDF",
};

const OFFICE_EXTS = [
  "doc", "docx", "rtf", "odt", "txt",
  "xls", "xlsx", "ods", "csv",
  "ppt", "pptx", "odp",
];

/** 每個工具吃什麼檔案。只有 JPG 轉 PDF 收圖片。 */
const ACCEPTS: Record<Tool, { name: string; extensions: string[] }> = {
  imagestopdf: { name: "JPEG 圖片", extensions: ["jpg", "jpeg"] },
  compress: { name: "PDF", extensions: ["pdf"] },
  merge: { name: "PDF", extensions: ["pdf"] },
  extract: { name: "PDF", extensions: ["pdf"] },
  remove: { name: "PDF", extensions: ["pdf"] },
  rotate: { name: "PDF", extensions: ["pdf"] },
  crop: { name: "PDF", extensions: ["pdf"] },
  split: { name: "PDF", extensions: ["pdf"] },
  toimage: { name: "PDF", extensions: ["pdf"] },
  repair: { name: "PDF", extensions: ["pdf"] },
  watermark: { name: "PDF", extensions: ["pdf"] },
  pagenumbers: { name: "PDF", extensions: ["pdf"] },
  officetopdf: { name: "Office 文件", extensions: OFFICE_EXTS },
};

/** 公釐轉 PDF 的點（1 pt = 1/72 吋）。 */
const MM_TO_PT = 72 / 25.4;

const num = (id: string) => Number($<HTMLInputElement>(id).value);

const fmt = (n: number) =>
  n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;

function setStatus(text: string) {
  statusEl.textContent = text;
}

/**
 * 解析 "1-3, 7, 10-12" 這類頁碼字串。
 * 回傳去重後的遞增頁碼；語法錯誤時丟出可直接顯示給使用者的訊息。
 */
function parsePages(spec: string, max: number): number[] {
  const out = new Set<number>();
  for (const chunk of spec.split(",").map((s) => s.trim()).filter(Boolean)) {
    const range = chunk.match(/^(\d+)\s*-\s*(\d+)$/);
    const single = chunk.match(/^(\d+)$/);
    if (range) {
      const [a, b] = [Number(range[1]), Number(range[2])];
      if (a < 1 || b < a) throw new Error(`頁碼範圍不正確：${chunk}`);
      if (b > max) throw new Error(`第 ${b} 頁不存在（本檔共 ${max} 頁）`);
      for (let i = a; i <= b; i++) out.add(i);
    } else if (single) {
      const n = Number(single[1]);
      if (n < 1) throw new Error(`頁碼不正確：${chunk}`);
      if (n > max) throw new Error(`第 ${n} 頁不存在（本檔共 ${max} 頁）`);
      out.add(n);
    } else {
      throw new Error(`看不懂的頁碼寫法：${chunk}`);
    }
  }
  return [...out].sort((a, b) => a - b);
}

/** 組出要送給後端的操作描述。輸入不合法時丟出錯誤，由呼叫端顯示。 */
function buildOperation(): object {
  const minPages = Math.min(...files.map((f) => f.pages ?? 0));
  switch (tool) {
    case "compress": {
      const level = document.querySelector<HTMLInputElement>('input[name="level"]:checked')!
        .value as Level;
      return { kind: "compress", level };
    }
    case "merge":
      return { kind: "merge" };
    case "extract": {
      const spec = $<HTMLInputElement>("extractPages").value.trim();
      if (!spec) throw new Error("請輸入要保留的頁碼");
      return { kind: "extract", pages: parsePages(spec, minPages) };
    }
    case "remove": {
      const spec = $<HTMLInputElement>("removePages").value.trim();
      if (!spec) throw new Error("請輸入要移除的頁碼");
      return { kind: "remove", pages: parsePages(spec, minPages) };
    }
    case "rotate": {
      const spec = $<HTMLInputElement>("rotatePages").value.trim();
      const degrees = Number(
        document.querySelector<HTMLInputElement>('input[name="angle"]:checked')!.value,
      );
      return { kind: "rotate", pages: spec ? parsePages(spec, minPages) : [], degrees };
    }
    case "crop": {
      const spec = $<HTMLInputElement>("cropPages").value.trim();
      const mm = {
        top: num("cropTop"),
        bottom: num("cropBottom"),
        left: num("cropLeft"),
        right: num("cropRight"),
      };
      if (Object.values(mm).some((v) => !Number.isFinite(v) || v < 0)) {
        throw new Error("邊界必須是 0 或正數");
      }
      if (Object.values(mm).every((v) => v === 0)) throw new Error("請至少設定一邊的裁切量");
      return {
        kind: "crop",
        pages: spec ? parsePages(spec, minPages) : [],
        margins: {
          top: mm.top * MM_TO_PT,
          bottom: mm.bottom * MM_TO_PT,
          left: mm.left * MM_TO_PT,
          right: mm.right * MM_TO_PT,
        },
      };
    }
    case "split": {
      const chunk = num("splitChunk");
      if (!Number.isInteger(chunk) || chunk < 1) throw new Error("每份頁數必須是 1 以上的整數");
      return { kind: "split", chunk };
    }
    case "toimage": {
      const dpi = num("imageDpi");
      if (!Number.isInteger(dpi) || dpi < 20 || dpi > 600) {
        throw new Error("解析度請設在 20 到 600 DPI 之間");
      }
      const format = document.querySelector<HTMLInputElement>('input[name="imgfmt"]:checked')!
        .value;
      return { kind: "toImage", dpi, format };
    }
    case "imagestopdf": {
      const mm = num("imgMargin");
      if (!Number.isFinite(mm) || mm < 0) throw new Error("留白必須是 0 或正數");
      return { kind: "imagesToPdf", margin: mm * MM_TO_PT };
    }
    case "repair":
      return { kind: "repair" };
    case "officetopdf":
      return { kind: "officeToPdf" };
    case "watermark": {
      const text = $<HTMLInputElement>("wmText").value.trim();
      if (!text) throw new Error("請輸入浮水印文字");
      const spec = $<HTMLInputElement>("wmPages").value.trim();
      return {
        kind: "watermark",
        text,
        style: styleFrom("wmFont", "wmSize", "wmColor", num("wmOpacity")),
        angle: num("wmAngle"),
        pages: spec ? parsePages(spec, minPages) : [],
      };
    }
    case "pagenumbers": {
      const template = $<HTMLInputElement>("pnTemplate").value.trim();
      if (!template) throw new Error("請輸入頁碼格式");
      const margin = num("pnMargin");
      if (!Number.isFinite(margin) || margin < 0) throw new Error("邊界必須是 0 或正數");
      const spec = $<HTMLInputElement>("pnPages").value.trim();
      return {
        kind: "pageNumbers",
        template,
        style: styleFrom("pnFont", "pnSize", "pnColor", 1),
        corner: $<HTMLSelectElement>("pnCorner").value,
        start_at: num("pnStart"),
        margin: margin * MM_TO_PT,
        pages: spec ? parsePages(spec, minPages) : [],
      };
    }
  }
}

/** `#rrggbb` → PDF 用的 0–1 三分量。 */
function hexToRgb(hex: string): [number, number, number] {
  const m = /^#?([0-9a-f]{6})$/i.exec(hex.trim());
  if (!m) throw new Error(`看不懂的色碼：${hex}`);
  const n = parseInt(m[1], 16);
  return [((n >> 16) & 255) / 255, ((n >> 8) & 255) / 255, (n & 255) / 255];
}

/** 組出後端 TextStyle 需要的欄位。欄位名是 snake_case，serde 不會幫忙轉。 */
function styleFrom(fontSel: string, sizeId: string, colorId: string, opacity: number) {
  const opt = $<HTMLSelectElement>(fontSel).selectedOptions[0];
  if (!opt) throw new Error("找不到可用的字型");
  const size = num(sizeId);
  if (!Number.isFinite(size) || size < 1 || size > 400) throw new Error("字級請設在 1 到 400 之間");
  return {
    font_path: opt.value,
    font_index: Number(opt.dataset.index ?? 0),
    size,
    color: hexToRgb($<HTMLInputElement>(colorId).value),
    opacity,
  };
}

const accepted = (path: string) =>
  ACCEPTS[tool].extensions.some((e) => path.toLowerCase().endsWith(`.${e}`));

async function addFiles(paths: string[]) {
  const keep = paths.filter(accepted);
  const skipped = paths.length - keep.length;
  const fresh = keep.filter((p) => !files.some((f) => f.path === p));
  if (fresh.length) {
    files = [...files, ...(await invoke<FileInfo[]>("inspect", { paths: fresh }))];
  }
  render();
  if (skipped > 0) {
    setStatus(`已略過 ${skipped} 個檔案：「${LABEL[tool]}」只接受 ${ACCEPTS[tool].name}`);
  }
}

function move(index: number, delta: number) {
  const to = index + delta;
  if (to < 0 || to >= files.length) return;
  [files[index], files[to]] = [files[to], files[index]];
  render();
}

function renderPending() {
  files.forEach((f, i) => {
    const li = document.createElement("li");
    li.className = f.error ? "item failed" : "item pending";

    const name = document.createElement("span");
    name.className = "name";
    name.textContent = f.name;

    const meta = document.createElement("span");
    meta.className = f.error ? "meta error" : "meta";
    // 圖片沒有頁數可言。
    meta.textContent =
      f.error ?? (f.pages === null ? fmt(f.bytes) : `${fmt(f.bytes)}　${f.pages} 頁`);
    li.append(name, meta);

    // 這兩個工具的輸出順序就是清單順序，所以需要調序。
    if ((tool === "merge" || tool === "imagestopdf") && !f.error) {
      for (const [glyph, delta] of [["↑", -1], ["↓", 1]] as const) {
        const b = document.createElement("button");
        b.type = "button";
        b.className = "ghost tiny";
        b.textContent = glyph;
        b.disabled = delta < 0 ? i === 0 : i === files.length - 1;
        b.onclick = () => move(i, delta);
        li.append(b);
      }
    }

    const drop = document.createElement("button");
    drop.type = "button";
    drop.className = "ghost tiny";
    drop.textContent = "✕";
    drop.title = "從清單移除";
    drop.onclick = () => {
      files = files.filter((x) => x.path !== f.path);
      render();
    };
    li.append(drop);
    listEl.append(li);
  });
}

function renderResults() {
  for (const r of results) {
    const li = document.createElement("li");
    li.className = r.error ? "item failed" : "item done";

    const name = document.createElement("span");
    name.className = "name";
    name.textContent = r.name;
    li.append(name);

    if (r.error) {
      const meta = document.createElement("span");
      meta.className = "meta error";
      meta.textContent = r.error;
      li.append(meta);
    } else {
      const saved = 1 - r.out_bytes / Math.max(1, r.orig_bytes);
      const meta = document.createElement("span");
      meta.className = "meta";
      // 壓縮才需要強調省了多少；其他工具改大小不是重點。
      meta.innerHTML =
        saved > 0.01
          ? `<s>${fmt(r.orig_bytes)}</s> → <b>${fmt(r.out_bytes)}</b>` +
            `<i class="good">省下 ${(saved * 100).toFixed(0)}%</i>`
          : `<span class="flat">${fmt(r.orig_bytes)} → ${fmt(r.out_bytes)}</span>`;

      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = "儲存…";
      btn.onclick = () => saveOne(r);
      li.append(meta, btn);
    }
    listEl.append(li);
  }
}

function render() {
  listEl.innerHTML = "";
  renderPending();
  renderResults();

  for (const el of document.querySelectorAll<HTMLElement>("[data-for]")) {
    el.hidden = el.dataset.for !== tool;
  }
  for (const b of toolsEl.querySelectorAll<HTMLButtonElement>("button")) {
    b.classList.toggle("active", b.dataset.tool === tool);
  }

  const usable = files.filter((f) => !f.error).length;
  runBtn.disabled = busy || usable === 0 || (tool === "merge" && usable < 2);
  runBtn.textContent = busy ? "處理中…" : LABEL[tool];
  saveAllBtn.hidden = !results.some((r) => !r.error);
  clearBtn.hidden = results.length === 0 && files.length === 0;
}

async function runOp() {
  if (busy || files.length === 0) return;
  let op: object;
  try {
    op = buildOperation();
  } catch (e) {
    setStatus(e instanceof Error ? e.message : String(e));
    return;
  }

  busy = true;
  render();
  const batch = files.filter((f) => !f.error).map((f) => f.path);
  try {
    const out = await invoke<JobResult[]>("run_op", { paths: batch, op });
    results = [...results, ...out];
    files = [];
    const ok = out.filter((r) => !r.error).length;
    setStatus(`完成 ${ok} / ${out.length}。尚未儲存的結果只存在暫存區。`);
  } catch (e) {
    setStatus(String(e));
  } finally {
    busy = false;
    render();
  }
}

/** 後端已經算好含副檔名的建議檔名，前端不再自己加後綴。 */
function saveFilters(name: string) {
  const ext = name.split(".").pop()?.toLowerCase() ?? "pdf";
  const known: Record<string, { name: string; extensions: string[] }> = {
    jpg: { name: "JPEG 圖片", extensions: ["jpg"] },
    png: { name: "PNG 圖片", extensions: ["png"] },
  };
  return [known[ext] ?? { name: "PDF", extensions: ["pdf"] }];
}

async function saveOne(r: JobResult) {
  const dest = await save({ defaultPath: r.name, filters: saveFilters(r.name) });
  if (!dest) return;
  try {
    await invoke("save_job", { id: r.id, dest });
    results = results.filter((x) => x.id !== r.id);
    setStatus(`已儲存，暫存檔已抹除：${dest}`);
  } catch (e) {
    setStatus(String(e));
  }
  render();
}

async function saveAll() {
  const dir = await open({ directory: true, multiple: false, title: "選擇儲存資料夾" });
  if (typeof dir !== "string") return;
  let saved = 0;
  for (const r of results.filter((x) => !x.error)) {
    try {
      await invoke("save_job", { id: r.id, dest: `${dir}\\${r.name}` });
      results = results.filter((x) => x.id !== r.id);
      saved++;
    } catch (e) {
      setStatus(String(e));
    }
  }
  setStatus(`已儲存 ${saved} 個檔案到 ${dir}，暫存檔已全部抹除。`);
  render();
}

async function clearAll() {
  await invoke("discard_all");
  files = [];
  results = [];
  setStatus("已清除所有暫存結果。");
  render();
}

async function browse() {
  const picked = await open({ multiple: true, filters: [ACCEPTS[tool]] });
  if (Array.isArray(picked)) await addFiles(picked);
  else if (typeof picked === "string") await addFiles([picked]);
}

toolsEl.onclick = (e) => {
  const btn = (e.target as HTMLElement).closest<HTMLButtonElement>("button[data-tool]");
  if (!btn || busy) return;
  tool = btn.dataset.tool as Tool;

  // 換工具後檔案類型可能不再適用（例如載入 JPG 後切到「壓縮」），先剔掉不合的。
  const before = files.length;
  files = files.filter((f) => accepted(f.path));
  setStatus(
    files.length < before
      ? `已移除 ${before - files.length} 個檔案：「${LABEL[tool]}」只接受 ${ACCEPTS[tool].name}`
      : "",
  );
  render();
};

runBtn.onclick = runOp;
saveAllBtn.onclick = saveAll;
clearBtn.onclick = clearAll;
$<HTMLButtonElement>("browse").onclick = browse;

getCurrentWebview().onDragDropEvent((e) => {
  if (e.payload.type === "over") {
    dropEl.classList.add("over");
  } else if (e.payload.type === "drop") {
    dropEl.classList.remove("over");
    void addFiles(e.payload.paths);
  } else {
    dropEl.classList.remove("over");
  }
});

listen<{ index: number; total: number; name: string }>("op:progress", (e) => {
  setStatus(`處理中 ${e.payload.index + 1}/${e.payload.total}：${e.payload.name}`);
});

interface FontChoice {
  label: string;
  path: string;
  index: number;
}

async function loadFonts() {
  const fonts = await invoke<FontChoice[]>("list_fonts");
  for (const sel of document.querySelectorAll<HTMLSelectElement>("select.fontpick")) {
    sel.innerHTML = "";
    for (const f of fonts) {
      const opt = document.createElement("option");
      opt.value = f.path;
      opt.dataset.index = String(f.index);
      opt.textContent = f.label;
      sel.append(opt);
    }
  }
  if (fonts.length === 0) setStatus("找不到可用的字型，浮水印與頁碼無法使用。");
}

interface Engines {
  ghostscript: string | null;
  libreoffice: string | null;
}

async function showEngines() {
  const e = await invoke<Engines>("engine_status");
  const parts: string[] = [];
  parts.push(e.ghostscript ? `Ghostscript ${e.ghostscript}` : "Ghostscript 缺失（壓縮／轉圖／修復無法使用）");
  // LibreOffice 不隨程式打包，沒有是正常情況，不該顯示成錯誤。
  parts.push(e.libreoffice ? e.libreoffice : "未偵測到 LibreOffice（Office 轉檔無法使用）");
  engineEl.textContent = parts.join("　·　");
  engineEl.className = e.ghostscript ? "engine ok" : "engine bad";
}

void loadFonts();
void showEngines();
render();
