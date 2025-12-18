"""產生壓縮測試語料。這是拋棄式的量測工具，不是產品程式碼。

用法: python bench/make_corpus.py
產出: bench/corpus/*.pdf
"""
import io
import math
import random
from pathlib import Path

from PIL import Image, ImageDraw, ImageFilter, ImageFont
from reportlab.lib.pagesizes import A4
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas

OUT = Path(__file__).parent / "corpus"
OUT.mkdir(parents=True, exist_ok=True)
random.seed(42)

CJK_TTF = r"C:\Windows\Fonts\kaiu.ttf"      # 標楷體，單一字體檔好處理
LATIN_TTF = r"C:\Windows\Fonts\arial.ttf"

PARA = (
    "本文件為壓縮測試語料，內容並無實質意義。中文字型內嵌會顯著影響檔案大小，"
    "因此語料必須包含足量的中文字元，才能反映實際使用情境下的壓縮表現。"
    "The quick brown fox jumps over the lazy dog. 0123456789. "
)


def c1_text(pages=100):
    """純文字論文型：壓縮空間主要來自字型子集化與物件流。"""
    pdfmetrics.registerFont(TTFont("CJK", CJK_TTF))
    path = OUT / "c1_text.pdf"
    c = canvas.Canvas(str(path), pagesize=A4)
    w, h = A4
    for p in range(pages):
        c.setFont("CJK", 16)
        c.drawString(60, h - 60, f"第 {p + 1} 章　壓縮測試文件")
        c.setFont("CJK", 10.5)
        y = h - 95
        for line in range(42):
            start = (p * 42 + line) * 17 % len(PARA)
            c.drawString(60, y, (PARA * 3)[start:start + 46])
            y -= 16
        c.showPage()
    c.save()
    return path


def _scan_page(idx, dpi=300, w_in=8.27, h_in=11.69):
    """模擬掃描頁：白底 + 文字 + 雜訊 + 輕微歪斜，存成 JPEG。"""
    w, h = int(w_in * dpi), int(h_in * dpi)
    img = Image.new("L", (w, h), 250)
    d = ImageDraw.Draw(img)
    font = ImageFont.truetype(CJK_TTF, 40)
    y = 300
    for line in range(38):
        start = (idx * 38 + line) * 13 % len(PARA)
        d.text((300, y), (PARA * 3)[start:start + 34], fill=30, font=font)
        y += 62
    img = img.rotate(random.uniform(-0.6, 0.6), fillcolor=250, resample=Image.BILINEAR)
    img = img.filter(ImageFilter.GaussianBlur(0.6))
    # 掃描雜訊：這是掃描件壓縮不下去的主因
    noise = Image.effect_noise((w, h), 12).convert("L")
    img = Image.blend(img, noise, 0.10)
    return img.convert("RGB")


def c2_scan(pages=30):
    """掃描影像型：最能拉開壓縮差距，也是使用者最常抱怨檔案太大的類型。"""
    path = OUT / "c2_scan.pdf"
    imgs = [_scan_page(i) for i in range(pages)]
    imgs[0].save(path, save_all=True, append_images=imgs[1:],
                 resolution=300.0, quality=92)
    return path


def _photo(w=1600, h=1200):
    """合成照片：漸層 + 色塊 + 雜訊，逼近真實照片的頻譜特性。"""
    img = Image.new("RGB", (w, h))
    px = img.load()
    ax, ay = random.uniform(2, 6), random.uniform(2, 6)
    base = [random.randint(40, 200) for _ in range(3)]
    for y in range(h):
        for x in range(0, w, 4):
            v = math.sin(x / w * ax * math.pi) * math.cos(y / h * ay * math.pi)
            c = tuple(min(255, max(0, int(b + v * 70))) for b in base)
            for k in range(4):
                if x + k < w:
                    px[x + k, y] = c
    d = ImageDraw.Draw(img)
    for _ in range(25):
        x0, y0 = random.randint(0, w), random.randint(0, h)
        d.ellipse([x0, y0, x0 + random.randint(40, 300), y0 + random.randint(40, 300)],
                  fill=tuple(random.randint(0, 255) for _ in range(3)))
    img = img.filter(ImageFilter.GaussianBlur(1.2))
    noise = Image.effect_noise((w, h), 20).convert("L").convert("RGB")
    return Image.blend(img, noise, 0.08)


def c3_mixed(pages=20):
    """圖文混排型錄：一般使用者最常見的簡報/型錄。"""
    pdfmetrics.registerFont(TTFont("CJK", CJK_TTF))
    path = OUT / "c3_mixed.pdf"
    c = canvas.Canvas(str(path), pagesize=A4)
    w, h = A4
    for p in range(pages):
        buf = io.BytesIO()
        _photo().save(buf, format="JPEG", quality=95)
        buf.seek(0)
        from reportlab.lib.utils import ImageReader
        c.drawImage(ImageReader(buf), 40, h - 340, width=w - 80, height=280)
        c.setFont("CJK", 18)
        c.drawString(40, h - 380, f"產品型號 A-{1000 + p}")
        c.setFont("CJK", 10.5)
        y = h - 410
        for line in range(18):
            start = (p * 18 + line) * 19 % len(PARA)
            c.drawString(40, y, (PARA * 3)[start:start + 48])
            y -= 15
        c.showPage()
    c.save()
    return path


if __name__ == "__main__":
    for fn in (c1_text, c2_scan, c3_mixed):
        p = fn()
        print(f"{p.name:16} {p.stat().st_size / 1024 / 1024:8.2f} MB")
