"""量 PDF → Word 的保真度。

「跑版」是主觀說法，要能改善就得先能量。作法是把 PDF 轉成 docx，再用 LibreOffice
轉回 PDF，然後逐頁跟原始檔比對 SSIM——來回一趟之後畫面差多少，就是跑版的程度。

同時列出 docx 裡參照到的字型與圖片錨定方式，這兩者是跑版最常見的來源：
* 字型：pdf2docx 直接沿用 PDF 裡的字型名稱（常是子集名如 DFKaiShu-SB-Estd-BF），
  系統沒有這個字型時 Word 會替換，字寬一變整段就重排。
* 圖片：浮動錨定（anchor）比行內（inline）容易跑掉。

用系統 Python 執行（需要 numpy / scipy / Pillow），pdf2docx 那一步以子行程呼叫
擴充包裡的 Python——這也和程式實際的呼叫方式一致。

用法: python bench/word_fidelity.py [語料.pdf]
"""
import collections
import logging
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

logging.disable(logging.INFO)

ROOT = Path(__file__).parent.parent
CORPUS = ROOT / "bench" / "corpus"
PAGES = 3
DPI = 100


def soffice() -> str | None:
    for base in (os.environ.get("ProgramFiles", ""), os.environ.get("ProgramFiles(x86)", "")):
        p = Path(base) / "LibreOffice/program/soffice.com"
        if p.exists():
            return str(p)
    return None


def ghostscript() -> str | None:
    p = ROOT / "vendor/ghostscript/bin/gswin64c.exe"
    return str(p) if p.exists() else None


def pyext_python() -> str | None:
    p = ROOT / "vendor/pyext/python.exe"
    return str(p) if p.exists() else None


# 和 src-tauri/src/pyext.rs 用的是同一段腳本，額外吃一個 pages 參數方便只轉前幾頁。
CONVERT = """
import sys, logging
logging.disable(logging.INFO)
from pdf2docx import Converter
cv = Converter(sys.argv[1])
try:
    cv.convert(sys.argv[2], pages=list(range(int(sys.argv[3]))))
finally:
    cv.close()
"""


def to_docx(py: str, src: Path, dst: Path, pages: int) -> None:
    result = subprocess.run(
        [py, "-c", CONVERT, str(src), str(dst), str(pages)],
        capture_output=True, text=True,
    )
    if result.returncode != 0 or not dst.exists():
        raise RuntimeError((result.stderr or "轉檔失敗").strip()[-300:])


def inspect_docx(path: Path) -> dict:
    xml = zipfile.ZipFile(path).read("word/document.xml").decode("utf-8")
    fonts = collections.Counter(
        re.findall(r'w:ascii="([^"]+)"', xml) + re.findall(r'w:eastAsia="([^"]+)"', xml)
    )
    return {
        "fonts": dict(fonts.most_common(6)),
        "inline_images": len(re.findall(r"<wp:inline", xml)),
        "floating_images": len(re.findall(r"<wp:anchor", xml)),
        "text_boxes": len(re.findall(r"<w:txbxContent>", xml)),
        "paragraphs": len(re.findall(r"<w:p[ >]", xml)),
    }


def render(gs: str, pdf: Path, out_dir: Path, pages: int) -> list[Path]:
    out_dir.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [gs, "-dSAFER", "-dBATCH", "-dNOPAUSE", "-dQUIET", "-sDEVICE=png16m",
         f"-r{DPI}", "-dFirstPage=1", f"-dLastPage={pages}",
         f"-sOutputFile={out_dir}/p%03d.png", str(pdf)],
        check=True, capture_output=True)
    return sorted(out_dir.glob("p*.png"))


def ssim(a, b) -> float:
    import numpy as np
    from scipy.ndimage import uniform_filter
    a, b = a.astype(np.float64), b.astype(np.float64)
    c1, c2 = (0.01 * 255) ** 2, (0.03 * 255) ** 2
    win = 7
    mu_a, mu_b = uniform_filter(a, win), uniform_filter(b, win)
    saa = uniform_filter(a * a, win) - mu_a * mu_a
    sbb = uniform_filter(b * b, win) - mu_b * mu_b
    sab = uniform_filter(a * b, win) - mu_a * mu_b
    num = (2 * mu_a * mu_b + c1) * (2 * sab + c2)
    den = (mu_a**2 + mu_b**2 + c1) * (saa + sbb + c2)
    return float(np.mean(num / den))


def compare(gs: str, original: Path, roundtrip: Path, work: Path) -> list[float]:
    from PIL import Image
    import numpy as np

    ref = render(gs, original, work / "ref", PAGES)
    got = render(gs, roundtrip, work / "got", PAGES)
    scores = []
    for r, g in zip(ref, got):
        ia = np.asarray(Image.open(r).convert("L"))
        ib = np.asarray(Image.open(g).convert("L"))
        if ia.shape != ib.shape:
            ib = np.asarray(Image.open(g).convert("L").resize(ia.shape[::-1]))
        scores.append(ssim(ia, ib))
    return scores


def main(sources: list[Path]):
    gs, so, py = ghostscript(), soffice(), pyext_python()
    if not gs or not so or not py:
        sys.exit("需要 vendor/ghostscript、vendor/pyext 與 LibreOffice")

    for src in sources:
        if not src.exists():
            print(f"{src.name}: 找不到")
            continue
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            docx = work / "mid.docx"
            to_docx(py, src, docx, PAGES)

            info = inspect_docx(docx)
            subprocess.run(
                [so, f"-env:UserInstallation=file:///{work.as_posix()}/profile",
                 "--headless", "--norestore", "--convert-to", "pdf",
                 "--outdir", str(work), str(docx)],
                check=True, capture_output=True)
            back = work / "mid.pdf"
            if not back.exists():
                print(f"{src.name}: 轉不回 PDF")
                continue

            scores = compare(gs, src, back, work)
            avg = sum(scores) / len(scores) if scores else float("nan")
            print(f"\n=== {src.name} ===")
            print(f"來回後 SSIM: 平均 {avg:.4f}  各頁 {[f'{s:.3f}' for s in scores]}")
            print(f"字型參照 : {info['fonts']}")
            print(f"圖片     : 行內 {info['inline_images']}　浮動 {info['floating_images']}")
            print(f"段落 {info['paragraphs']}　文字方塊 {info['text_boxes']}")
            shutil.rmtree(work / "ref", ignore_errors=True)
            shutil.rmtree(work / "got", ignore_errors=True)


if __name__ == "__main__":
    args = [Path(a) for a in sys.argv[1:]] or [
        CORPUS / "c3_mixed.pdf",
        CORPUS / "c1_text.pdf",
    ]
    main(args)
