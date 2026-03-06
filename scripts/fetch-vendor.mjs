/**
 * 取得隨程式打包的第三方執行檔，放到 vendor/。
 *
 *   node scripts/fetch-vendor.mjs                # 全部
 *   node scripts/fetch-vendor.mjs qpdf           # 只取 qpdf
 *   node scripts/fetch-vendor.mjs ghostscript    # 只取 Ghostscript
 *
 * 這些檔案不進版控（每個平台不同、而且是幾十 MB 的二進位檔），一律用這支腳本重現。
 * 已經是正確版本就跳過，可以放心重複執行。
 *
 * | 引擎        | Windows                                  | macOS                              |
 * |-------------|------------------------------------------|------------------------------------|
 * | qpdf        | 官方 msvc64.zip                          | 官方 bin-macos-{arm64,x86_64}.zip  |
 * | Ghostscript | 官方 NSIS 安裝器，用 7-Zip 解開，只留 bin/ | 官方沒有二進位檔，從原始碼編譯      |
 *
 * macOS 預設取本機架構；要打包另一個架構時設 LOCALPDF_ARCH=arm64 或 x64。
 */
import { spawnSync } from "node:child_process";
import {
  chmodSync,
  copyFileSync,
  existsSync,
  mkdirSync,
  readdirSync,
  renameSync,
  rmSync,
  statSync,
  writeFileSync,
} from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const VENDOR = join(ROOT, "vendor");
const CACHE = join(VENDOR, "_cache");

// 改版本請同步更新 vendor/README.md；Ghostscript 改版還要重跑 bench/compress_bench.py。
const QPDF_VERSION = "12.4.2";
const GS_VERSION = "10.07.1";
const GS_TAG = "gs10071";
const SEVENZIP_MSI = "https://github.com/ip7z/7zip/releases/download/26.02/7z2602-x64.msi";

const PLATFORM = process.platform;
const ARCH = process.env.LOCALPDF_ARCH ?? process.arch;

function log(msg) {
  console.log(`[fetch-vendor] ${msg}`);
}

async function download(url, dest) {
  mkdirSync(dirname(dest), { recursive: true });
  if (existsSync(dest)) {
    log(`已快取 ${dest.replace(ROOT, ".")}`);
    return dest;
  }
  log(`下載 ${url}`);
  const res = await fetch(url);
  if (!res.ok) throw new Error(`下載失敗 ${res.status}：${url}`);
  const part = `${dest}.part`;
  writeFileSync(part, Buffer.from(await res.arrayBuffer()));
  renameSync(part, dest);
  return dest;
}

function run(cmd, args, opts = {}) {
  const r = spawnSync(cmd, args, { stdio: "inherit", ...opts });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`指令失敗（${r.status}）：${cmd} ${args.join(" ")}`);
}

function capture(cmd, args) {
  const r = spawnSync(cmd, args, { encoding: "utf8" });
  return r.status === 0 ? r.stdout.trim() : "";
}

/** Windows 10 起內建的 bsdtar 能解 zip；Git Bash 的 GNU tar 不行，所以指定完整路徑。 */
function tar() {
  return PLATFORM === "win32" ? join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe") : "tar";
}

function extract(archive, dest) {
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dest, { recursive: true });
  run(tar(), ["-xf", archive, "-C", dest]);
}

/** 在解開的目錄裡找某個檔案（官方壓縮檔的頂層資料夾名稱會隨版本變）。 */
function findFile(dir, name) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const p = join(dir, entry.name);
    if (entry.isDirectory()) {
      const hit = findFile(p, name);
      if (hit) return hit;
    } else if (entry.name === name) {
      return p;
    }
  }
  return null;
}

function sizeMB(dir) {
  let total = 0;
  const walk = (d) => {
    for (const e of readdirSync(d, { withFileTypes: true })) {
      const p = join(d, e.name);
      if (e.isDirectory()) walk(p);
      else total += statSync(p).size;
    }
  };
  walk(dir);
  return (total / 1048576).toFixed(1);
}

// ───────────────────────────── qpdf ─────────────────────────────

async function qpdf() {
  const dest = join(VENDOR, "qpdf");
  const exe = PLATFORM === "win32" ? join(dest, "qpdf.exe") : join(dest, "bin", "qpdf");
  if (existsSync(exe) && capture(exe, ["--version"]).includes(QPDF_VERSION)) {
    log(`qpdf ${QPDF_VERSION} 已就緒`);
    return;
  }
  rmSync(dest, { recursive: true, force: true });
  mkdirSync(dest, { recursive: true });
  const base = `https://github.com/qpdf/qpdf/releases/download/v${QPDF_VERSION}`;

  if (PLATFORM === "win32") {
    const zip = await download(`${base}/qpdf-${QPDF_VERSION}-msvc64.zip`, join(CACHE, `qpdf-${QPDF_VERSION}-msvc64.zip`));
    const raw = join(CACHE, "qpdf-win");
    extract(zip, raw);
    const bin = dirname(findFile(raw, "qpdf.exe"));
    // zlib-flate 與 fix-qdf 是獨立小工具，程式沒用到。MSVC 執行期 DLL 必須留著，
    // 否則在沒裝 Visual C++ Redistributable 的機器上會啟動失敗。
    for (const name of readdirSync(bin)) {
      if (!/\.(exe|dll)$/i.test(name) || /zlib-flate|fix-qdf|qpdf-tools/i.test(name)) continue;
      copyFileSync(join(bin, name), join(dest, name));
    }
  } else if (PLATFORM === "darwin") {
    const arch = ARCH === "arm64" ? "arm64" : "x86_64";
    const name = `qpdf-${QPDF_VERSION}-bin-macos-${arch}.zip`;
    const zip = await download(`${base}/${name}`, join(CACHE, name));
    const raw = join(CACHE, `qpdf-mac-${arch}`);
    extract(zip, raw);
    // 保留官方的 bin/ + lib/ 結構：qpdf 以 @executable_path/../lib 找函式庫。
    // libqpdf.30.dylib 在壓縮檔裡是符號連結，copyFileSync 會複製實體內容——
    // 打包工具不一定保留符號連結，實體檔最保險。
    mkdirSync(join(dest, "bin"));
    mkdirSync(join(dest, "lib"));
    copyFileSync(join(raw, "bin", "qpdf"), exe);
    chmodSync(exe, 0o755);
    copyFileSync(join(raw, "lib", "libqpdf.30.dylib"), join(dest, "lib", "libqpdf.30.dylib"));
  } else {
    throw new Error(`qpdf：不支援的平台 ${PLATFORM}`);
  }

  const ver = capture(exe, ["--version"]);
  if (!ver.includes(QPDF_VERSION)) throw new Error(`qpdf 驗證失敗，輸出：${ver || "（無法執行）"}`);
  log(`qpdf 完成：${ver.split(/\r?\n/)[0]}，${sizeMB(dest)} MB`);
}

// ────────────────────────── Ghostscript ─────────────────────────

/**
 * 官方 Windows 版以 COMPILE_INITS 編譯：lib/、Resource/、iccprofiles/ 已內建在 DLL 裡。
 * 實測只留 bin/ 時，bench 語料的壓縮、出圖、文字抽取結果與完整安裝逐位元組相同。
 */
const GS_UNUSED = ["lib", "Resource", "iccprofiles"];

async function ghostscript() {
  const dest = join(VENDOR, "ghostscript");
  const exe = join(dest, "bin", PLATFORM === "win32" ? "gswin64c.exe" : "gs");

  if (existsSync(exe) && capture(exe, ["--version"]) === GS_VERSION) {
    for (const d of GS_UNUSED) {
      if (existsSync(join(dest, d))) {
        rmSync(join(dest, d), { recursive: true, force: true });
        log(`移除用不到的 ghostscript/${d}/`);
      }
    }
    log(`Ghostscript ${GS_VERSION} 已就緒（${sizeMB(dest)} MB）`);
    return;
  }

  if (PLATFORM === "win32") {
    const installer = await download(
      `https://github.com/ArtifexSoftware/ghostpdl-downloads/releases/download/${GS_TAG}/${GS_TAG}w64.exe`,
      join(CACHE, `${GS_TAG}w64.exe`),
    );
    // 官方只提供 NSIS 安裝器，但它其實是壓縮包，用 7-Zip 解開即可，不必真的安裝。
    // msiexec 的 administrative install（/a）只是解開 MSI，不需要管理員權限。
    const sevenZip = join(CACHE, "7zx", "Files", "7-Zip", "7z.exe");
    if (!existsSync(sevenZip)) {
      const msi = await download(SEVENZIP_MSI, join(CACHE, "7z.msi"));
      run("msiexec", ["/a", msi, "/qn", `TARGETDIR=${join(CACHE, "7zx")}`]);
    }
    const raw = join(CACHE, "gs_raw");
    rmSync(raw, { recursive: true, force: true });
    run(sevenZip, ["x", installer, `-o${raw}`, "-y"], { stdio: "ignore" });

    rmSync(dest, { recursive: true, force: true });
    mkdirSync(join(dest, "bin"), { recursive: true });
    // 只要主控台版與 DLL；GUI 版 gswin64.exe 用不到
    for (const name of ["gswin64c.exe", "gsdll64.dll"]) {
      copyFileSync(join(raw, "bin", name), join(dest, "bin", name));
    }
  } else if (PLATFORM === "darwin") {
    run("bash", [join(ROOT, "scripts", "build-gs-macos.sh")], {
      env: { ...process.env, GS_VERSION, GS_TAG, ARCH: ARCH === "x64" ? "x86_64" : ARCH },
    });
  } else {
    throw new Error(`Ghostscript：不支援的平台 ${PLATFORM}`);
  }

  const ver = capture(exe, ["--version"]);
  if (ver !== GS_VERSION) throw new Error(`Ghostscript 驗證失敗，輸出：${ver || "（無法執行）"}`);
  log(`Ghostscript 完成：${ver}，${sizeMB(dest)} MB`);
}

// ─────────────────────────────────────────────────────────────────

const TASKS = { qpdf, ghostscript };
const wanted = process.argv.slice(2);
const names = wanted.length ? wanted : Object.keys(TASKS);

for (const name of names) {
  const task = TASKS[name];
  if (!task) {
    console.error(`不認識的項目：${name}（可用：${Object.keys(TASKS).join("、")}）`);
    process.exit(1);
  }
  await task();
}
