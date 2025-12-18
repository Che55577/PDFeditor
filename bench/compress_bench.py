"""量測 Ghostscript 各組壓縮參數的「檔案大小 vs 畫質」取捨。

這是拋棄式量測工具。它的產出（勝出的參數組）會被寫死進 Rust 核心，
所以這支程式的價值在於數據，不在於程式碼本身。

用法: python bench/compress_bench.py [語料檔...]
"""
import csv
import shutil
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
from PIL import Image
from scipy.ndimage import uniform_filter

ROOT = Path(__file__).parent
CORPUS = ROOT / "corpus"
WORK = ROOT / "_work"
SSIM_PAGES = 5        # 只比對前 N 頁，夠反映品質且快
SSIM_DPI = 110


def find_gs() -> str:
    """優先用專案內 vendor 的版本，其次系統安裝的。"""
    local = ROOT.parent / "vendor" / "ghostscript" / "bin" / "gswin64c.exe"
    if local.exists():
        return str(local)
    for base in (r"C:\Program Files\gs", r"C:\Program Files (x86)\gs"):
        p = Path(base)
        if p.exists():
            hits = sorted(p.glob("*/bin/gswin64c.exe"), reverse=True)
            if hits:
                return str(hits[0])
    if shutil.which("gswin64c"):
        return "gswin64c"
    sys.exit("找不到 Ghostscript，請先安裝或放到 vendor/ghostscript/")


GS = find_gs()

# -dSAFER：限制 PostScript 存取檔案系統。隱私工具必須開，不可省。
BASE = ["-dSAFER", "-dBATCH", "-dNOPAUSE", "-dQUIET", "-sDEVICE=pdfwrite",
        "-dCompatibilityLevel=1.7", "-dSubsetFonts=true", "-dCompressFonts=true",
        "-dDetectDuplicateImages=true"]


def dpi_profile(color, gray, mono, jpegq=None, cmyk2rgb=True):
    """自訂降採樣參數組。

    兩個關鍵修正（初版實測發現的坑，詳見 README）：
    * DownsampleThreshold 預設 1.5 → 來源沒超過目標 1.5 倍就整個跳過降採樣，
      造成「壓縮後大小完全沒變」。設成 1.0，只要超過目標就處理。
    * PassThroughJPEGImages 預設 true → 沒降採樣的 JPEG 原樣複製，JPEGQ 完全失效。
      關掉它，確保要求的畫質參數一定生效。
    """
    a = [
        "-dDownsampleColorImages=true", f"-dColorImageResolution={color}",
        "-dColorImageDownsampleType=/Bicubic", "-dColorImageDownsampleThreshold=1.0",
        "-dDownsampleGrayImages=true", f"-dGrayImageResolution={gray}",
        "-dGrayImageDownsampleType=/Bicubic", "-dGrayImageDownsampleThreshold=1.0",
        "-dDownsampleMonoImages=true", f"-dMonoImageResolution={mono}",
        "-dMonoImageDownsampleType=/Subsample", "-dMonoImageDownsampleThreshold=1.0",
        "-dPassThroughJPEGImages=false", "-dPassThroughJPXImages=false",
    ]
    if jpegq is not None:
        a += ["-dAutoFilterColorImages=false", "-dColorImageFilter=/DCTEncode",
              "-dAutoFilterGrayImages=false", "-dGrayImageFilter=/DCTEncode",
              f"-dJPEGQ={jpegq}"]
    if cmyk2rgb:
        a += ["-dConvertCMYKImagesToRGB=true"]
    return a


PROFILES = {
    # Ghostscript 內建預設，當作基準線。它們同樣有 threshold / passthrough 的坑。
    "preset_screen":   ["-dPDFSETTINGS=/screen"],
    "preset_ebook":    ["-dPDFSETTINGS=/ebook"],
    "preset_printer":  ["-dPDFSETTINGS=/printer"],
    # 候選：極限壓縮
    "x_72_q40":        dpi_profile(72, 72, 300, jpegq=40),
    "x_96_q50":        dpi_profile(96, 96, 300, jpegq=50),
    # 候選：建議壓縮（預設值，最重要）
    "r_144_q65":       dpi_profile(144, 144, 600, jpegq=65),
    "r_150_q75":       dpi_profile(150, 150, 600, jpegq=75),
    "r_180_q80":       dpi_profile(180, 180, 600, jpegq=80),
    # 候選：低壓縮 / 保列印品質。重點是「必須真的有壓到」，不能像初版一樣毫無作用。
    "l_225_q85":       dpi_profile(225, 225, 900, jpegq=85),
    "l_300_q88":       dpi_profile(300, 300, 1200, jpegq=88),
    # 候選：無損（刻意保留 passthrough，只重整結構與字型）
    "lossless":        ["-dDownsampleColorImages=false", "-dDownsampleGrayImages=false",
                        "-dDownsampleMonoImages=false", "-dAutoFilterColorImages=false",
                        "-dPassThroughJPEGImages=true"],
}


def render(pdf: Path, out_dir: Path, pages: int) -> list[Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [GS, "-dSAFER", "-dBATCH", "-dNOPAUSE", "-dQUIET", "-sDEVICE=png16m",
         f"-r{SSIM_DPI}", "-dFirstPage=1", f"-dLastPage={pages}",
         f"-sOutputFile={out_dir}/p%03d.png", str(pdf)],
        check=True, capture_output=True)
    return sorted(out_dir.glob("p*.png"))


def ssim(a: np.ndarray, b: np.ndarray) -> float:
    """標準 SSIM（uniform filter 版本），值域 0~1，1 = 完全相同。"""
    a, b = a.astype(np.float64), b.astype(np.float64)
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    win = 7
    mu_a, mu_b = uniform_filter(a, win), uniform_filter(b, win)
    saa = uniform_filter(a * a, win) - mu_a * mu_a
    sbb = uniform_filter(b * b, win) - mu_b * mu_b
    sab = uniform_filter(a * b, win) - mu_a * mu_b
    num = (2 * mu_a * mu_b + c1) * (2 * sab + c2)
    den = (mu_a ** 2 + mu_b ** 2 + c1) * (saa + sbb + c2)
    return float(np.mean(num / den))


def quality(orig: Path, comp: Path) -> float:
    ref = render(orig, WORK / "ref", SSIM_PAGES)
    got = render(comp, WORK / "cmp", SSIM_PAGES)
    scores = []
    for r, g in zip(ref, got):
        ia = np.asarray(Image.open(r).convert("L"))
        ib = np.asarray(Image.open(g).convert("L"))
        if ia.shape != ib.shape:
            ib = np.asarray(Image.open(g).convert("L").resize(ia.shape[::-1]))
        scores.append(ssim(ia, ib))
    shutil.rmtree(WORK / "ref", ignore_errors=True)
    shutil.rmtree(WORK / "cmp", ignore_errors=True)
    return sum(scores) / len(scores) if scores else float("nan")


def main(files: list[Path]):
    WORK.mkdir(exist_ok=True)
    rows = []
    for src in files:
        base = src.stat().st_size
        print(f"\n=== {src.name}  ({base / 1048576:.2f} MB) ===")
        print(f"{'profile':16} {'out MB':>8} {'ratio':>7} {'sec':>6} {'SSIM':>7}")
        for name, args in PROFILES.items():
            out = WORK / f"{src.stem}__{name}.pdf"
            t0 = time.perf_counter()
            p = subprocess.run([GS, *BASE, *args, f"-sOutputFile={out}", str(src)],
                               capture_output=True, text=True)
            dt = time.perf_counter() - t0
            if p.returncode != 0 or not out.exists():
                print(f"{name:16} {'FAIL':>8}  {p.stderr.strip()[:60]}")
                continue
            size = out.stat().st_size
            q = quality(src, out)
            print(f"{name:16} {size / 1048576:8.2f} {size / base:7.1%} {dt:6.1f} {q:7.4f}")
            rows.append({"corpus": src.name, "profile": name, "orig_bytes": base,
                         "out_bytes": size, "ratio": round(size / base, 4),
                         "seconds": round(dt, 2), "ssim": round(q, 4)})
            out.unlink()
    csv_path = ROOT / "results.csv"
    with csv_path.open("w", newline="", encoding="utf-8") as f:
        w = csv.DictWriter(f, fieldnames=list(rows[0].keys()))
        w.writeheader()
        w.writerows(rows)
    print(f"\n寫入 {csv_path}")


if __name__ == "__main__":
    args = [Path(a) for a in sys.argv[1:]] or sorted(CORPUS.glob("*.pdf"))
    print(f"Ghostscript: {GS}")
    main(args)
