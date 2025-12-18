"""把不同壓縮參數下的同一塊中文文字並排輸出，用眼睛判讀可讀性。

存在的理由：SSIM 量的是像素相似度，量不出「這行中文還讀不讀得懂」。
掃描件在低 dpi 下 SSIM 可能還有 0.88，但中文字已經糊成一團——那是功能失效，
不是畫質下降。挑「極限壓縮」的參數時必須用眼睛看過。

用法: python bench/legibility_check.py
產出: bench/legibility.png
"""
import subprocess
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

sys.path.insert(0, str(Path(__file__).parent))
from compress_bench import BASE, GS, WORK, dpi_profile  # noqa: E402

ROOT = Path(__file__).parent
SRC = ROOT / "corpus" / "c2_scan.pdf"
PAGE = 2
RENDER_DPI = 150
CROP = (150, 260, 1130, 560)   # 頁面左上一塊有內文的區域

CANDIDATES = {
    "原始 300dpi": None,
    "72dpi / JPEG q40": dpi_profile(72, 72, 300, jpegq=40),
    "96dpi / JPEG q50": dpi_profile(96, 96, 300, jpegq=50),
    "144dpi / JPEG q65": dpi_profile(144, 144, 600, jpegq=65),
}


def render_crop(pdf: Path) -> Image.Image:
    out = WORK / "leg"
    out.mkdir(parents=True, exist_ok=True)
    for old in out.glob("*.png"):
        old.unlink()
    subprocess.run(
        [GS, "-dSAFER", "-dBATCH", "-dNOPAUSE", "-dQUIET", "-sDEVICE=png16m",
         f"-r{RENDER_DPI}", f"-dFirstPage={PAGE}", f"-dLastPage={PAGE}",
         f"-sOutputFile={out}/p.png", str(pdf)],
        check=True, capture_output=True)
    return Image.open(out / "p.png").convert("RGB").crop(CROP)


def main():
    if not SRC.exists():
        sys.exit("找不到語料，請先跑 python bench/make_corpus.py")
    WORK.mkdir(exist_ok=True)
    font = ImageFont.truetype(r"C:\Windows\Fonts\msjh.ttc", 22)

    panels = []
    for label, args in CANDIDATES.items():
        if args is None:
            pdf = SRC
        else:
            pdf = WORK / "leg_candidate.pdf"
            subprocess.run([GS, *BASE, *args, f"-sOutputFile={pdf}", str(SRC)],
                           check=True, capture_output=True)
        size = pdf.stat().st_size / 1048576
        panels.append((f"{label}　—　{size:.2f} MB", render_crop(pdf)))

    w = max(p.width for _, p in panels)
    bar = 34
    total_h = sum(p.height + bar for _, p in panels)
    sheet = Image.new("RGB", (w, total_h), "white")
    d = ImageDraw.Draw(sheet)

    y = 0
    for label, img in panels:
        d.rectangle([0, y, w, y + bar], fill=(28, 32, 38))
        d.text((10, y + 5), label, fill="white", font=font)
        sheet.paste(img, (0, y + bar))
        y += bar + img.height

    dest = ROOT / "legibility.png"
    sheet.save(dest)
    print(f"寫入 {dest}")


if __name__ == "__main__":
    main()
