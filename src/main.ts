import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";

import { Editor, type CanvasPoint } from "./editor";
import { extractLines } from "./mupdf-text";

type Tool =
  | "compress" | "merge" | "extract" | "remove" | "rotate" | "crop" | "split"
  | "toimage" | "imagestopdf" | "repair" | "watermark" | "pagenumbers"
  | "stamp" | "officetopdf" | "tomarkdown" | "annotate";

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

type Group = "整理" | "最佳化" | "轉換" | "編輯";

interface ToolSpec {
  id: Tool;
  label: string;
  hint: string;
  /** 依用途分組，讓 16 個功能不要一次全攤在眼前 */
  group: Group;
  /** 後端的排序位置，UI 依此排列設定面板，讓畫面順序與實際執行順序一致 */
  order: number;
  /** 這個工具吃什麼副檔名；未指定代表 PDF */
  accepts?: { name: string; extensions: string[] };
  /** 不能和其他功能一起執行 */
  exclusive?: boolean;
  /** 產出非 PDF，後面不能再接東西 */
  terminal?: boolean;
}

const GROUPS: Group[] = ["整理", "最佳化", "轉換", "編輯"];

const OFFICE_EXTS = [
  "doc", "docx", "rtf", "odt", "txt",
  "xls", "xlsx", "ods", "csv",
  "ppt", "pptx", "odp",
];

/** `order` 刻意與 Rust 的 `Operation::order` 一致。 */
const TOOLS: ToolSpec[] = [
  { id: "merge", label: "合併", hint: "多份併成一份", group: "整理", order: 2 },
  { id: "extract", label: "擷取頁面", hint: "只保留指定頁", group: "整理", order: 3 },
  { id: "remove", label: "移除頁面", hint: "刪掉指定頁", group: "整理", order: 3 },
  { id: "rotate", label: "旋轉", hint: "90 / 180 / 270 度", group: "整理", order: 4 },
  { id: "crop", label: "裁切", hint: "縮減頁面邊界", group: "整理", order: 5 },
  { id: "split", label: "分割", hint: "每 N 頁一個檔", group: "整理", order: 10 },

  { id: "compress", label: "壓縮", hint: "縮小檔案大小", group: "最佳化", order: 9 },
  { id: "repair", label: "修復", hint: "重建損毀的檔案", group: "最佳化", order: 1 },

  { id: "officetopdf", label: "Office 轉 PDF", hint: "Word / Excel / PPT", group: "轉換", order: 0,
    accepts: { name: "Office 文件", extensions: OFFICE_EXTS } },
  { id: "imagestopdf", label: "圖片轉 PDF", hint: "JPG / PNG 併成一份", group: "轉換", order: 0,
    accepts: { name: "圖片", extensions: ["jpg", "jpeg", "png"] } },
  { id: "toimage", label: "轉成圖片", hint: "每頁一張 JPG / PNG", group: "轉換", order: 11, terminal: true },
  { id: "tomarkdown", label: "轉成 Markdown", hint: "抽出文字與結構", group: "轉換", order: 11, terminal: true },

  { id: "watermark", label: "浮水印", hint: "斜向文字", group: "編輯", order: 6 },
  { id: "pagenumbers", label: "頁碼", hint: "自動編號", group: "編輯", order: 7 },
  { id: "stamp", label: "圖章", hint: "簽名檔、公司章", group: "編輯", order: 8 },
  { id: "annotate", label: "註記", hint: "互動式編輯器", group: "編輯", order: 99, exclusive: true },
];

const spec = (id: Tool) => TOOLS.find((t) => t.id === id)!;
const PDF_ONLY = { name: "PDF", extensions: ["pdf"] };

const $ = <T extends HTMLElement>(id: string) => document.getElementById(id) as T;
const num = (id: string) => Number($<HTMLInputElement>(id).value);
/** 公釐轉 PDF 的點（1 pt = 1/72 吋）。 */
const MM_TO_PT = 72 / 25.4;

const dropEl = $<HTMLElement>("drop");
const filesEl = $<HTMLUListElement>("files");
const listEl = $<HTMLUListElement>("list");
const statusEl = $<HTMLElement>("status");
const engineEl = $<HTMLElement>("engine");
const toolsEl = $<HTMLElement>("tools");
const runBtn = $<HTMLButtonElement>("run");
const saveAllBtn = $<HTMLButtonElement>("saveAll");
const clearBtn = $<HTMLButtonElement>("clear");

/** 已勾選的功能。 */
const selected = new Set<Tool>();
/** 待處理的檔案。只存路徑與中繼資料，內容不進前端記憶體。 */
let files: FileInfo[] = [];
/** 已完成、尚未存檔的結果。 */
let results: JobResult[] = [];
let busy = false;
/** 圖章的圖片檔另外選，不進批次清單。 */
let stampImage: string | null = null;

const fmt = (n: number) =>
  n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;

function setStatus(text: string) {
  statusEl.textContent = text;
}

/** 目前接受的檔案類型：入口轉換工具說了算，否則就是 PDF。 */
function accepting(): { name: string; extensions: string[] } {
  for (const id of selected) {
    const s = spec(id);
    if (s.accepts) return s.accepts;
  }
  return PDF_ONLY;
}

const accepted = (path: string) =>
  accepting().extensions.some((e) => path.toLowerCase().endsWith(`.${e}`));

/** 依執行順序列出已勾選的功能。 */
const orderedSelection = (): ToolSpec[] =>
  TOOLS.filter((t) => selected.has(t.id)).sort((a, b) => a.order - b.order);

// ---- 檔案清單 ----

/** 路徑 → 縮圖的 blob URL。算過就留著，換工具重畫清單時不必再算一次。 */
const thumbs = new Map<string, string>();

/** 背景產生縮圖。失敗就算了——沒有縮圖只是少了預覽，不該打斷任何事。 */
async function loadThumb(path: string) {
  if (thumbs.has(path) || !path.toLowerCase().endsWith(".pdf")) return;
  try {
    const buf = await invoke<ArrayBuffer>("thumbnail", { path });
    thumbs.set(path, URL.createObjectURL(new Blob([buf], { type: "image/jpeg" })));
    render();
  } catch {
    /* 沒有縮圖不影響功能 */
  }
}

async function addFiles(paths: string[]) {
  const keep = paths.filter(accepted);
  const skipped = paths.length - keep.length;
  const fresh = keep.filter((p) => !files.some((f) => f.path === p));
  if (fresh.length) {
    files = [...files, ...(await invoke<FileInfo[]>("inspect", { paths: fresh }))];
    for (const p of fresh) void loadThumb(p);
  }
  render();
  if (skipped > 0) {
    setStatus(`已略過 ${skipped} 個檔案：目前只接受 ${accepting().name}`);
  }
}

function move(index: number, delta: number) {
  const to = index + delta;
  if (to < 0 || to >= files.length) return;
  [files[index], files[to]] = [files[to], files[index]];
  render();
}

function renderFiles() {
  filesEl.innerHTML = "";
  // 合併與圖片轉 PDF 的輸出順序就是這個清單的順序，所以需要調序按鈕。
  const ordered = selected.has("merge") || selected.has("imagestopdf");

  files.forEach((f, i) => {
    const li = document.createElement("li");
    li.className = f.error ? "filecard bad" : "filecard";

    const thumb = document.createElement("div");
    thumb.className = "thumb";
    const url = thumbs.get(f.path);
    if (url) {
      const img = document.createElement("img");
      img.src = url;
      img.alt = "";
      thumb.append(img);
    }
    li.append(thumb);

    const info = document.createElement("div");
    info.className = "fileinfo";
    const name = document.createElement("span");
    name.className = "filename";
    name.textContent = f.name;
    name.title = f.path;
    const meta = document.createElement("span");
    meta.className = f.error ? "filemeta error" : "filemeta";
    meta.textContent =
      f.error ?? (f.pages === null ? fmt(f.bytes) : `${fmt(f.bytes)}　·　${f.pages} 頁`);
    info.append(name, meta);
    li.append(info);

    if (ordered && !f.error) {
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
    filesEl.append(li);
  });
}

// ---- 功能勾選 ----

function buildToolCards() {
  toolsEl.innerHTML = "";
  for (const g of GROUPS) {
    const section = document.createElement("section");
    section.className = "toolgroup";
    section.dataset.group = g;

    const title = document.createElement("h3");
    title.textContent = g;
    const grid = document.createElement("div");
    grid.className = "tool-grid";

    for (const t of TOOLS.filter((x) => x.group === g)) {
      const label = document.createElement("label");
      label.className = "toolcard";
      label.dataset.tool = t.id;
      label.dataset.search = `${t.label} ${t.hint} ${t.id}`;

      const box = document.createElement("input");
      box.type = "checkbox";
      box.checked = selected.has(t.id);
      box.onchange = () => toggle(t.id, box.checked);

      const text = document.createElement("span");
      text.className = "tooltext";
      const name = document.createElement("strong");
      name.textContent = t.label;
      const hint = document.createElement("em");
      hint.textContent = t.hint;
      text.append(name, hint);

      label.append(box, text);
      grid.append(label);
    }
    section.append(title, grid);
    toolsEl.append(section);
  }
}

/** 依關鍵字篩選功能卡片；整組都沒東西時連標題一起收起來。 */
function filterTools(query: string) {
  const q = query.trim().toLowerCase();
  for (const section of toolsEl.querySelectorAll<HTMLElement>(".toolgroup")) {
    let visible = 0;
    for (const card of section.querySelectorAll<HTMLElement>(".toolcard")) {
      const hit = !q || (card.dataset.search ?? "").toLowerCase().includes(q);
      card.hidden = !hit;
      if (hit) visible++;
    }
    section.hidden = visible === 0;
  }
}

function toggle(id: Tool, on: boolean) {
  const s = spec(id);
  if (on) {
    // 註記是互動式的，不能和批次功能混用；反之亦然。
    if (s.exclusive) selected.clear();
    else for (const other of [...selected]) if (spec(other).exclusive) selected.delete(other);

    // 入口轉換工具彼此互斥，因為它們吃的來源格式不同。
    if (s.accepts) for (const other of [...selected]) if (spec(other).accepts) selected.delete(other);
    // 終點工具彼此互斥，它們都產生非 PDF 的輸出。
    if (s.terminal) for (const other of [...selected]) if (spec(other).terminal) selected.delete(other);
    // 擷取與移除是互斥的兩種說法
    if (id === "extract") selected.delete("remove");
    if (id === "remove") selected.delete("extract");
    if (s.terminal) selected.delete("split");
    if (id === "split") for (const other of [...selected]) if (spec(other).terminal) selected.delete(other);

    selected.add(id);
  } else {
    selected.delete(id);
  }

  // 換了功能之後，原本載入的檔案類型可能不再適用
  const before = files.length;
  files = files.filter((f) => accepted(f.path));
  if (files.length < before) {
    setStatus(`已移除 ${before - files.length} 個檔案：目前只接受 ${accepting().name}`);
  } else {
    setStatus("");
  }
  render();
}

// ---- 參數組裝 ----

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

const pagesOf = (id: string, max: number) => {
  const raw = $<HTMLInputElement>(id).value.trim();
  return raw ? parsePages(raw, max) : [];
};

/** 把一個已勾選的功能轉成後端的 Operation。 */
function operationFor(id: Tool, minPages: number): object {
  switch (id) {
    case "compress": {
      const level = document.querySelector<HTMLInputElement>('input[name="level"]:checked')!.value;
      return { kind: "compress", level };
    }
    case "merge":
      return { kind: "merge" };
    case "extract": {
      const raw = $<HTMLInputElement>("extractPages").value.trim();
      if (!raw) throw new Error("「擷取頁面」需要指定保留哪些頁");
      return { kind: "extract", pages: parsePages(raw, minPages) };
    }
    case "remove": {
      const raw = $<HTMLInputElement>("removePages").value.trim();
      if (!raw) throw new Error("「移除頁面」需要指定移除哪些頁");
      return { kind: "remove", pages: parsePages(raw, minPages) };
    }
    case "rotate": {
      const degrees = Number(
        document.querySelector<HTMLInputElement>('input[name="angle"]:checked')!.value,
      );
      return { kind: "rotate", pages: pagesOf("rotatePages", minPages), degrees };
    }
    case "crop": {
      const mm = {
        top: num("cropTop"), bottom: num("cropBottom"),
        left: num("cropLeft"), right: num("cropRight"),
      };
      if (Object.values(mm).some((v) => !Number.isFinite(v) || v < 0)) {
        throw new Error("裁切邊界必須是 0 或正數");
      }
      if (Object.values(mm).every((v) => v === 0)) throw new Error("「裁切」請至少設定一邊的裁切量");
      return {
        kind: "crop",
        pages: pagesOf("cropPages", minPages),
        margins: {
          top: mm.top * MM_TO_PT, bottom: mm.bottom * MM_TO_PT,
          left: mm.left * MM_TO_PT, right: mm.right * MM_TO_PT,
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
      const format = document.querySelector<HTMLInputElement>('input[name="imgfmt"]:checked')!.value;
      return { kind: "toImage", dpi, format };
    }
    case "imagestopdf": {
      const mm = num("imgMargin");
      if (!Number.isFinite(mm) || mm < 0) throw new Error("留白必須是 0 或正數");
      return { kind: "imagesToPdf", margin: mm * MM_TO_PT };
    }
    case "repair":
      return { kind: "repair" };
    case "watermark": {
      const text = $<HTMLInputElement>("wmText").value.trim();
      if (!text) throw new Error("請輸入浮水印文字");
      return {
        kind: "watermark",
        text,
        style: styleFrom("wmFont", "wmSize", "wmColor", num("wmOpacity")),
        angle: num("wmAngle"),
        pages: pagesOf("wmPages", minPages),
      };
    }
    case "pagenumbers": {
      const template = $<HTMLInputElement>("pnTemplate").value.trim();
      if (!template) throw new Error("請輸入頁碼格式");
      const margin = num("pnMargin");
      if (!Number.isFinite(margin) || margin < 0) throw new Error("頁碼邊界必須是 0 或正數");
      return {
        kind: "pageNumbers",
        template,
        style: styleFrom("pnFont", "pnSize", "pnColor", 1),
        corner: $<HTMLSelectElement>("pnCorner").value,
        start_at: num("pnStart"),
        margin: margin * MM_TO_PT,
        pages: pagesOf("pnPages", minPages),
      };
    }
    case "stamp": {
      if (!stampImage) throw new Error("請先選擇圖章圖片");
      const width = num("stampWidth");
      const margin = num("stampMargin");
      const opacity = num("stampOpacity");
      if (!Number.isFinite(width) || width < 1) throw new Error("圖章寬度必須大於 0");
      if (!Number.isFinite(margin) || margin < 0) throw new Error("圖章邊界必須是 0 或正數");
      if (!Number.isFinite(opacity) || opacity < 0 || opacity > 1) {
        throw new Error("圖章透明度必須介於 0 與 1 之間");
      }
      return {
        kind: "stamp",
        image: stampImage,
        place: {
          corner: $<HTMLSelectElement>("stampCorner").value,
          margin: margin * MM_TO_PT,
          width: width * MM_TO_PT,
          opacity,
        },
        pages: pagesOf("stampPages", minPages),
      };
    }
    case "officetopdf":
      return { kind: "officeToPdf" };
    case "tomarkdown":
      return { kind: "toMarkdown" };
    case "annotate":
      throw new Error("註記是互動式的，不走批次流程");
  }
}

// ---- 執行 ----

/**
 * Markdown 走的路和其他工具不同：檔案內容讀進 WebView，交給 MuPDF 抽出**帶字級**的
 * 結構化文字，Rust 只負責整理成 Markdown 並寫進暫存區。
 *
 * 這麼做是因為 Ghostscript 的文字抽取拿不到字級，標題層級只能靠行寬猜。
 * MuPDF 載入失敗時退回原本那條路——標題會比較不準，但至少還能用。
 */
async function runMarkdown(paths: string[]): Promise<JobResult[]> {
  const out: JobResult[] = [];
  for (const [i, path] of paths.entries()) {
    const name = path.split(/[\\/]/).pop() ?? path;
    setStatus(`處理中 ${i + 1}/${paths.length}：${name}`);
    try {
      const buf = await invoke<ArrayBuffer>("read_file_bytes", { path });
      const lines = await extractLines(new Uint8Array(buf));
      out.push(await invoke<JobResult>("markdown_from_lines", { name, lines }));
    } catch (e) {
      const detail = e instanceof Error ? e.message : String(e);
      setStatus(`${name}：改用備援方式抽取（${detail}）`);
      out.push(
        ...(await invoke<JobResult[]>("run_op", { paths: [path], op: { kind: "toMarkdown" } })),
      );
    }
  }
  return out;
}

async function runAll() {
  if (busy || files.length === 0 || selected.size === 0) return;
  const chosen = orderedSelection();
  const usable = files.filter((f) => !f.error).map((f) => f.path);
  if (usable.length === 0) return;

  if (chosen.some((t) => t.id === "annotate")) {
    busy = true;
    render();
    try {
      await edOpen(usable[0]);
      files = [];
    } catch (e) {
      setStatus(e instanceof Error ? e.message : String(e));
    } finally {
      busy = false;
      render();
    }
    return;
  }

  const minPages = Math.min(...files.map((f) => f.pages ?? Number.MAX_SAFE_INTEGER));
  let ops: object[];
  try {
    ops = chosen.map((t) => operationFor(t.id, minPages));
  } catch (e) {
    setStatus(e instanceof Error ? e.message : String(e));
    return;
  }

  busy = true;
  render();
  try {
    // Markdown 需要 MuPDF 才有字級，所以單獨走前端那條路。
    const out =
      chosen.length === 1 && chosen[0].id === "tomarkdown"
        ? await runMarkdown(usable)
        : await invoke<JobResult[]>("run_pipeline", { paths: usable, ops });
    results = [...results, ...out];
    files = [];
    setStatus(`完成 ${out.length} 個輸出。尚未儲存的結果只存在暫存區。`);
  } catch (e) {
    setStatus(e instanceof Error ? e.message : String(e));
  } finally {
    busy = false;
    render();
  }
}

// ---- 結果與儲存 ----

function saveFilters(name: string) {
  const ext = name.split(".").pop()?.toLowerCase() ?? "pdf";
  const known: Record<string, { name: string; extensions: string[] }> = {
    jpg: { name: "JPEG 圖片", extensions: ["jpg"] },
    png: { name: "PNG 圖片", extensions: ["png"] },
    md: { name: "Markdown", extensions: ["md"] },
  };
  return [known[ext] ?? PDF_ONLY];
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

function renderResults() {
  listEl.innerHTML = "";
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
      meta.innerHTML =
        r.orig_bytes > 0 && saved > 0.01
          ? `<s>${fmt(r.orig_bytes)}</s> → <b>${fmt(r.out_bytes)}</b>` +
            `<i class="good">省下 ${(saved * 100).toFixed(0)}%</i>`
          : `<span class="flat">${fmt(r.out_bytes)}</span>`;
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
  renderFiles();
  renderResults();

  for (const el of document.querySelectorAll<HTMLElement>("[data-for]")) {
    const id = el.dataset.for as Tool;
    el.hidden = !selected.has(id);
    // 設定面板依實際執行順序排列，畫面看到的順序就是處理的順序。
    el.style.order = String(spec(id).order);
  }
  for (const card of toolsEl.querySelectorAll<HTMLElement>(".toolcard")) {
    const on = selected.has(card.dataset.tool as Tool);
    card.classList.toggle("on", on);
    // 互斥規則會在背後取消其他選項，勾選框必須跟著回來，
    // 否則使用者會看到兩個都打勾卻只有一個生效。
    const box = card.querySelector<HTMLInputElement>("input")!;
    if (box.checked !== on) box.checked = on;
  }

  const chosen = orderedSelection();
  const usable = files.filter((f) => !f.error).length;
  const needsTwo = selected.has("merge") && usable < 2;

  // 收合狀態下也要看得到選了什麼，所以 chips 放在 summary 裡。
  const chipsEl = $<HTMLElement>("chips");
  chipsEl.innerHTML = "";
  if (chosen.length === 0) {
    const empty = document.createElement("span");
    empty.className = "chips-empty";
    empty.textContent = "尚未選擇";
    chipsEl.append(empty);
  } else {
    chosen.forEach((t, i) => {
      if (i > 0) {
        const arrow = document.createElement("span");
        arrow.className = "chip-arrow";
        arrow.textContent = "→";
        chipsEl.append(arrow);
      }
      const chip = document.createElement("span");
      chip.className = "chip";
      chip.textContent = t.label;
      chipsEl.append(chip);
    });
  }

  runBtn.disabled =
    busy || usable === 0 || chosen.length === 0 || needsTwo || (selected.has("annotate") && !!editor);
  runBtn.textContent = busy ? "處理中…" : selected.has("annotate") ? "開啟編輯器" : "執行";

  $<HTMLElement>("plan").textContent =
    usable === 0
      ? `先加入檔案（目前接受 ${accepting().name}）`
      : chosen.length === 0
        ? "已加入檔案，接著勾選功能"
        : needsTwo
          ? "「合併」至少需要兩個檔案"
          : selected.has("annotate")
            ? `對 ${files[0]?.name ?? "檔案"} 開啟編輯器`
            : `${usable} 個檔案　·　${chosen.map((t) => t.label).join(" → ")}`;

  $<HTMLElement>("acceptHint").textContent = `　（目前接受 ${accepting().name}）`;
  saveAllBtn.hidden = !results.some((r) => !r.error);
  clearBtn.hidden = results.length === 0 && files.length === 0;
  dropEl.classList.toggle("compact", files.length > 0);
}

// ---- 註記編輯器 ----

const edEls = {
  panel: $<HTMLElement>("editor"),
  canvas: $<HTMLCanvasElement>("edCanvas"),
  input: $<HTMLInputElement>("edInput"),
  page: $<HTMLElement>("edPage"),
  zoom: $<HTMLElement>("edZoom"),
  hint: $<HTMLElement>("edHint"),
  mode: $<HTMLElement>("edMode"),
};

type EdMode = "text" | "ink" | "redact";

let editor: Editor | null = null;
let edPage = 0;
let edMode: EdMode = "text";
let edSource = "";
let edStrokes: CanvasPoint[][] = [];
let edDrawing = false;
/** 塗黑框選的起點 */
let edRectFrom: CanvasPoint | null = null;

const edColor = (): [number, number, number] => hexToRgb($<HTMLInputElement>("edColor").value);
const edSize = () => Math.min(120, Math.max(6, num("edSize")));

/** 滑鼠事件轉畫布像素。畫布可能被 CSS 縮放過，所以要用 rect 換算。 */
function edPoint(e: PointerEvent): CanvasPoint {
  const c = edEls.canvas;
  const rect = c.getBoundingClientRect();
  return {
    x: (e.clientX - rect.left) * (c.width / rect.width),
    y: (e.clientY - rect.top) * (c.height / rect.height),
  };
}

async function edRender() {
  if (!editor) return;
  const info = await editor.renderTo(edEls.canvas, edPage);
  edPage = info.index;
  edEls.page.textContent = `${info.index + 1} / ${info.total}`;
  edEls.zoom.textContent = `${Math.round(editor.scale * 100)}%`;
  edEls.hint.textContent = editor.dirty ? "有未儲存的變更" : "";
}

async function edOpen(path: string) {
  edSource = path.split(/[\\/]/).pop() ?? path;
  setStatus(`載入 ${edSource}…`);
  const buf = await invoke<ArrayBuffer>("read_file_bytes", { path });
  editor = await Editor.open(new Uint8Array(buf));
  edPage = 0;
  edEls.panel.hidden = false;
  await edRender();
  setStatus(`已開啟 ${edSource}，共 ${editor.pageCount} 頁。`);
  render();
}

function edClose() {
  editor?.destroy();
  editor = null;
  edStrokes = [];
  edDrawing = false;
  edEls.panel.hidden = true;
  edEls.input.hidden = true;
  render();
}

const MODE_HINT: Record<EdMode, string> = {
  text: "文字模式：點一下頁面任意處，輸入文字後按 Enter。",
  ink: "手繪模式：在頁面上按住拖曳。放開時筆跡才會寫入，畫面會重畫一次。",
  redact: "塗黑模式：拖出一塊矩形標記要移除的內容。存檔時會從檔案真的刪除，無法還原。",
};

function edSetMode(mode: EdMode) {
  edMode = mode;
  for (const [id, m] of [["edModeText", "text"], ["edModeInk", "ink"], ["edModeRedact", "redact"]] as const) {
    $<HTMLButtonElement>(id).classList.toggle("active", mode === m);
  }
  edEls.mode.textContent = MODE_HINT[mode];
  edEls.mode.classList.toggle("danger", mode === "redact");
}

/** 在點擊處浮出輸入框，Enter 送出、Esc 取消。 */
function edPromptText(at: CanvasPoint, clientX: number, clientY: number) {
  const stage = edEls.canvas.parentElement!;
  const box = stage.getBoundingClientRect();
  const input = edEls.input;
  input.value = "";
  input.hidden = false;
  input.style.left = `${clientX - box.left + stage.scrollLeft}px`;
  input.style.top = `${clientY - box.top + stage.scrollTop}px`;
  input.focus();

  const finish = async (commit: boolean) => {
    input.hidden = true;
    input.onkeydown = null;
    input.onblur = null;
    if (!commit || !input.value.trim() || !editor) return;
    try {
      editor.addText(at, input.value, { size: edSize(), color: edColor() });
      await edRender();
      setStatus("已加入文字註記。");
    } catch (e) {
      setStatus(`加入文字失敗：${e instanceof Error ? e.message : String(e)}`);
    }
  };
  input.onkeydown = (ev) => {
    if (ev.key === "Enter") void finish(true);
    else if (ev.key === "Escape") void finish(false);
  };
  input.onblur = () => void finish(false);
}

/** 把目前累積的筆跡寫成註記。失敗一定要說出來，不能靜默丟掉。 */
async function edCommitInk() {
  if (!editor) return;
  const strokes = edStrokes;
  edStrokes = [];
  if (strokes.length === 0) return;
  try {
    editor.addInk(strokes, { width: Math.max(1, edSize() / 8), color: edColor() });
    await edRender();
    setStatus(`已加入手繪筆跡（${strokes[0].length} 個點）。`);
  } catch (e) {
    setStatus(`手繪寫入失敗：${e instanceof Error ? e.message : String(e)}`);
    await edRender();
  }
}

/** 把框選的區域標記成塗黑。 */
async function edCommitRedaction(to: CanvasPoint) {
  const from = edRectFrom;
  edRectFrom = null;
  if (!from || !editor) return;
  try {
    editor.markRedaction(from, to);
    await edRender();
    setStatus(`已標記 ${editor.redactionCount} 塊塗黑區域。存檔時才會真的移除。`);
  } catch (e) {
    setStatus(`標記塗黑失敗：${e instanceof Error ? e.message : String(e)}`);
    await edRender();
  }
}

edEls.canvas.onpointerdown = (e) => {
  if (!editor) return;
  const p = edPoint(e);
  if (edMode === "text") {
    edPromptText(p, e.clientX, e.clientY);
    return;
  }
  if (edMode === "redact") {
    edRectFrom = p;
  } else {
    edStrokes = [[p]];
  }
  edDrawing = true;
  try {
    edEls.canvas.setPointerCapture(e.pointerId);
  } catch {
    // 沒有指標捕捉也能畫，只是拖出畫布外會斷；不值得因此中斷操作
  }
};

edEls.canvas.onpointermove = (e) => {
  if (!edDrawing || !editor) return;
  const p = edPoint(e);
  const ctx = edEls.canvas.getContext("2d");
  if (!ctx) return;

  if (edMode === "redact") {
    if (!edRectFrom) return;
    // 每次都重畫整頁再疊上目前的框，否則會留下一堆殘影
    void editor.renderTo(edEls.canvas, edPage).then(() => {
      const c = edEls.canvas.getContext("2d");
      if (!c || !edRectFrom) return;
      c.fillStyle = "rgb(0 0 0 / 0.55)";
      c.strokeStyle = "rgb(200 0 0)";
      c.lineWidth = 2;
      const x = Math.min(edRectFrom.x, p.x);
      const y = Math.min(edRectFrom.y, p.y);
      const w = Math.abs(p.x - edRectFrom.x);
      const h = Math.abs(p.y - edRectFrom.y);
      c.fillRect(x, y, w, h);
      c.strokeRect(x, y, w, h);
    });
    return;
  }

  const stroke = edStrokes[edStrokes.length - 1];
  stroke.push(p);
  // 即時預覽；正式的筆跡在放開時才寫進註記，之後重畫會取代這條線。
  if (stroke.length < 2) return;
  const [r, g, b] = edColor();
  ctx.strokeStyle = `rgb(${r * 255} ${g * 255} ${b * 255})`;
  ctx.lineWidth = Math.max(1, edSize() / 8) * editor.scale;
  ctx.lineCap = "round";
  ctx.lineJoin = "round";
  const prev = stroke[stroke.length - 2];
  ctx.beginPath();
  ctx.moveTo(prev.x, prev.y);
  ctx.lineTo(p.x, p.y);
  ctx.stroke();
};

/** pointerup 有可能因指標捕捉中斷而不觸發，所以另外兩個事件也要收尾。 */
function edFinishDrag(e?: PointerEvent) {
  if (!edDrawing) return;
  edDrawing = false;
  if (edMode === "redact") {
    void edCommitRedaction(e ? edPoint(e) : (edRectFrom ?? { x: 0, y: 0 }));
  } else {
    void edCommitInk();
  }
}

edEls.canvas.onpointerup = (e) => edFinishDrag(e);
edEls.canvas.onpointercancel = () => edFinishDrag();
edEls.canvas.onlostpointercapture = () => edFinishDrag();

$<HTMLButtonElement>("edPrev").onclick = () => {
  edPage = Math.max(0, edPage - 1);
  void edRender();
};
$<HTMLButtonElement>("edNext").onclick = () => {
  edPage = Math.min((editor?.pageCount ?? 1) - 1, edPage + 1);
  void edRender();
};
$<HTMLButtonElement>("edZoomIn").onclick = () => {
  editor?.setScale((editor?.scale ?? 1) * 1.25);
  void edRender();
};
$<HTMLButtonElement>("edZoomOut").onclick = () => {
  editor?.setScale((editor?.scale ?? 1) / 1.25);
  void edRender();
};
$<HTMLButtonElement>("edUndo").onclick = async () => {
  if (!editor?.undoLast()) {
    setStatus("這一頁沒有可以移除的註記。");
    return;
  }
  await edRender();
  setStatus("已移除最後加入的註記。");
};
$<HTMLButtonElement>("edModeText").onclick = () => edSetMode("text");
$<HTMLButtonElement>("edModeInk").onclick = () => edSetMode("ink");
$<HTMLButtonElement>("edModeRedact").onclick = () => edSetMode("redact");
$<HTMLButtonElement>("edClose").onclick = () => {
  edClose();
  setStatus("已關閉編輯器，未儲存的變更已丟棄。");
};
$<HTMLButtonElement>("edSave").onclick = async () => {
  if (!editor) return;
  const redacted = editor.redactionCount;
  try {
    const bytes = editor.save();
    // Rust 那側只收 base64；直接傳 Uint8Array 會變成 JSON 數字陣列。
    let binary = "";
    for (const b of bytes) binary += String.fromCharCode(b);
    const result = await invoke<JobResult>("save_edited", {
      name: edSource,
      dataBase64: btoa(binary),
    });
    results = [...results, result];
    edClose();
    setStatus(
      redacted > 0
        ? `已套用 ${redacted} 塊塗黑並完整重寫檔案，被塗掉的內容不會留在檔案裡。請在下方清單儲存。`
        : "已產生註記後的檔案，請在下方清單儲存。尚未存檔前只存在暫存區。",
    );
  } catch (e) {
    setStatus(e instanceof Error ? e.message : String(e));
  }
};

// ---- 其餘接線 ----

runBtn.onclick = runAll;
saveAllBtn.onclick = saveAll;
clearBtn.onclick = async () => {
  await invoke("discard_all");
  files = [];
  results = [];
  setStatus("已清除所有暫存結果。");
  render();
};

$<HTMLButtonElement>("browse").onclick = async () => {
  const picked = await open({ multiple: true, filters: [accepting()] });
  if (Array.isArray(picked)) await addFiles(picked);
  else if (typeof picked === "string") await addFiles([picked]);
};

$<HTMLButtonElement>("stampPick").onclick = async () => {
  const picked = await open({
    multiple: false,
    filters: [{ name: "圖片", extensions: ["png", "jpg", "jpeg"] }],
  });
  if (typeof picked !== "string") return;
  stampImage = picked;
  $<HTMLElement>("stampName").textContent = picked.split(/[\\/]/).pop() ?? picked;
  render();
};

// 這兩段接的是宿主環境的 API。它們在模組頂層執行，一旦拋錯，後面所有初始化
// （包括建立功能卡片）都不會跑，介面會變成一個沒有反應的空殼。包起來讓它安靜失敗，
// 使用者至少還能用「選擇檔案」。
try {
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
} catch (e) {
  console.warn("無法註冊拖放事件，改用「選擇檔案」：", e);
}

try {
  listen<{ index: number; total: number; name: string }>("op:progress", (e) => {
    const { index, total, name } = e.payload;
    setStatus(total > 1 ? `步驟 ${index + 1}/${total}：${name}` : `處理中：${name}`);
  });
} catch (e) {
  console.warn("無法註冊進度事件：", e);
}

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
  const parts = [
    e.ghostscript ? `Ghostscript ${e.ghostscript}` : "Ghostscript 缺失（壓縮／轉圖／修復無法使用）",
    // LibreOffice 不隨程式打包，沒有是正常情況，不該顯示成錯誤。
    e.libreoffice ?? "未偵測到 LibreOffice（Office 轉檔無法使用）",
  ];
  engineEl.textContent = parts.join("　·　");
  engineEl.className = e.ghostscript ? "engine ok" : "engine bad";
}

$<HTMLInputElement>("toolFilter").oninput = (e) =>
  filterTools((e.target as HTMLInputElement).value);

buildToolCards();
edSetMode("text");
void loadFonts();
void showEngines();
render();
