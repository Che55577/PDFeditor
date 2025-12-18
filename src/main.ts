import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open, save } from "@tauri-apps/plugin-dialog";

type Level = "extreme" | "recommended" | "low";

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
const runBtn = $<HTMLButtonElement>("run");
const saveAllBtn = $<HTMLButtonElement>("saveAll");
const clearBtn = $<HTMLButtonElement>("clear");

/** 待壓縮的來源檔路徑。只存路徑，檔案內容不進前端記憶體。 */
let queued: string[] = [];
/** 已完成、還沒存檔的結果。每一筆在後端都對應一個暫存檔。 */
let results: JobResult[] = [];
let busy = false;

const fmt = (n: number) =>
  n >= 1048576 ? `${(n / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;

const currentLevel = (): Level =>
  document.querySelector<HTMLInputElement>('input[name="level"]:checked')!.value as Level;

const outName = (name: string) => name.replace(/(\.pdf)?$/i, "") + "_compressed.pdf";

function setStatus(text: string) {
  statusEl.textContent = text;
}

function addFiles(paths: string[]) {
  const pdfs = paths.filter((p) => p.toLowerCase().endsWith(".pdf"));
  const skipped = paths.length - pdfs.length;
  for (const p of pdfs) if (!queued.includes(p)) queued.push(p);
  render();
  if (skipped > 0) setStatus(`已略過 ${skipped} 個非 PDF 檔案`);
}

function render() {
  listEl.innerHTML = "";

  for (const path of queued) {
    const li = document.createElement("li");
    li.className = "item pending";
    const name = document.createElement("span");
    name.className = "name";
    name.textContent = path.split(/[\\/]/).pop() ?? path;
    const meta = document.createElement("span");
    meta.className = "meta";
    meta.textContent = "待壓縮";
    li.append(name, meta);
    listEl.append(li);
  }

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
      // 已經最佳化過的 PDF 再壓會變大。這種情況要講清楚，不能包裝成戰果。
      const noGain = saved <= 0.01;
      const meta = document.createElement("span");
      meta.className = "meta";
      meta.innerHTML = noGain
        ? `<span class="flat">已無壓縮空間（${fmt(r.orig_bytes)} → ${fmt(r.out_bytes)}）</span>`
        : `<s>${fmt(r.orig_bytes)}</s> → <b>${fmt(r.out_bytes)}</b>` +
          `<i class="good">省下 ${(saved * 100).toFixed(0)}%</i>`;
      const btn = document.createElement("button");
      btn.type = "button";
      btn.textContent = "儲存…";
      if (noGain) btn.className = "ghost";
      btn.onclick = () => saveOne(r);
      li.append(meta, btn);
    }
    listEl.append(li);
  }

  runBtn.disabled = busy || queued.length === 0;
  runBtn.textContent = busy ? "壓縮中…" : `開始壓縮${queued.length ? `（${queued.length}）` : ""}`;
  saveAllBtn.hidden = !results.some((r) => !r.error);
  clearBtn.hidden = results.length === 0 && queued.length === 0;
}

async function runCompress() {
  if (busy || queued.length === 0) return;
  busy = true;
  const batch = queued;
  queued = [];
  render();

  try {
    const out = await invoke<JobResult[]>("compress_files", {
      paths: batch,
      level: currentLevel(),
    });
    results = [...results, ...out];
    const ok = out.filter((r) => !r.error).length;
    setStatus(`完成 ${ok} / ${out.length} 個檔案。尚未儲存的結果只存在暫存區。`);
  } catch (e) {
    setStatus(String(e));
    queued = batch; // 整批失敗時把檔案還回佇列，使用者不必重選
  } finally {
    busy = false;
    render();
  }
}

async function saveOne(r: JobResult) {
  const dest = await save({
    defaultPath: outName(r.name),
    filters: [{ name: "PDF", extensions: ["pdf"] }],
  });
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
      await invoke("save_job", { id: r.id, dest: `${dir}\\${outName(r.name)}` });
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
  queued = [];
  results = [];
  setStatus("已清除所有暫存結果。");
  render();
}

async function browse() {
  const picked = await open({
    multiple: true,
    filters: [{ name: "PDF", extensions: ["pdf"] }],
  });
  if (Array.isArray(picked)) addFiles(picked);
  else if (typeof picked === "string") addFiles([picked]);
}

runBtn.onclick = runCompress;
saveAllBtn.onclick = saveAll;
clearBtn.onclick = clearAll;
$<HTMLButtonElement>("browse").onclick = browse;

getCurrentWebview().onDragDropEvent((e) => {
  if (e.payload.type === "over") {
    dropEl.classList.add("over");
  } else if (e.payload.type === "drop") {
    dropEl.classList.remove("over");
    addFiles(e.payload.paths);
  } else {
    dropEl.classList.remove("over");
  }
});

listen<{ index: number; total: number; name: string }>("compress:progress", (e) => {
  setStatus(`壓縮中 ${e.payload.index + 1}/${e.payload.total}：${e.payload.name}`);
});

invoke<string>("engine_status")
  .then((v) => {
    engineEl.textContent = `壓縮引擎就緒（Ghostscript ${v}）`;
    engineEl.className = "engine ok";
  })
  .catch((e) => {
    engineEl.textContent = `${e} － 壓縮功能無法使用`;
    engineEl.className = "engine bad";
  });

render();
