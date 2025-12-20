"""PDF 轉 Word。主程式與 bench 共用這一支，避免兩邊行為飄移。

用法: python scripts/pdf2word.py <來源.pdf> <輸出.docx> [設定 JSON]

## 為什麼需要「向量前處理」

pdf2docx 的資料模型只有三種東西：文字、點陣圖、以及能變成表格框或底色的簡單形狀。
**任意向量圖形無處可去**——條碼、QR 碼、向量商標這類東西會直接消失。

所以在交給 pdf2docx 之前，先把向量密集的區域算成點陣圖疊回原處。pdf2docx 看到的
就是一張普通圖片，自然會保留。

判斷「密集」用的是路徑數量而不是面積：條碼是上百條細長矩形擠在一小塊區域，
而表格框線是少數幾條橫跨整頁的線——後者必須放過，否則表格會變成圖片。
"""
import json
import logging
import sys
from pathlib import Path

logging.disable(logging.INFO)

# 一個群集要有幾條路徑才算「向量圖形」而不是版面線條。
MIN_PATHS_PER_CLUSTER = 8
# 群集之間的間距門檻（pt）。條碼的線條間距遠小於此。
CLUSTER_GAP = 12.0
# 點陣化的解析度。條碼要夠清晰才掃得動。
RASTER_DPI = 300
# 太大的群集不處理——那多半是整頁的框線，不是圖形。
MAX_CLUSTER_FRACTION = 0.55


def _merge(a, b):
    return (min(a[0], b[0]), min(a[1], b[1]), max(a[2], b[2]), max(a[3], b[3]))


def _touches(a, b, gap):
    return not (
        a[2] + gap < b[0] or b[2] + gap < a[0] or a[3] + gap < b[1] or b[3] + gap < a[1]
    )


def cluster(rects, gap=CLUSTER_GAP):
    """把彼此靠近的矩形合併成群集，回傳 (bbox, 路徑數) 清單。"""
    groups: list[tuple[tuple, int]] = []
    for r in rects:
        box = (r.x0, r.y0, r.x1, r.y1)
        hits = [i for i, (g, _) in enumerate(groups) if _touches(g, box, gap)]
        if not hits:
            groups.append((box, 1))
            continue
        merged, count = groups[hits[0]], 0
        for i in sorted(hits, reverse=True):
            g, n = groups.pop(i)
            merged = (_merge(merged[0], g), 0)
            count += n
        groups.append((_merge(merged[0], box), count + 1))
    return groups


def rasterize_vector_art(doc, dpi=RASTER_DPI) -> int:
    """把向量密集的區域算成點陣圖疊回原處。回傳處理了幾塊。"""
    import pymupdf

    done = 0
    for page in doc:
        drawings = page.get_drawings()
        if not drawings:
            continue
        page_area = abs(page.rect.width * page.rect.height)
        groups = cluster([d["rect"] for d in drawings])

        for box, count in groups:
            if count < MIN_PATHS_PER_CLUSTER:
                continue  # 少數幾條線，是版面框線不是圖形
            rect = pymupdf.Rect(box)
            if rect.width < 2 or rect.height < 2:
                continue
            if abs(rect.width * rect.height) > page_area * MAX_CLUSTER_FRACTION:
                continue  # 太大，多半是整頁的框線

            pix = page.get_pixmap(clip=rect, dpi=dpi)
            # 疊在原本的向量圖形上。原圖形會被 pdf2docx 丟掉，不會重複出現。
            page.insert_image(rect, pixmap=pix, overlay=True)
            done += 1
    return done


def convert(src: Path, dst: Path, settings: dict) -> None:
    import pymupdf
    from pdf2docx import Converter

    keep_vector = settings.pop("keep_vector_art", True)
    pages = settings.pop("pages", None)

    work = src
    tmp = None
    if keep_vector:
        doc = pymupdf.open(src)
        try:
            if rasterize_vector_art(doc):
                tmp = dst.with_suffix(".prep.pdf")
                doc.save(tmp)
                work = tmp
        finally:
            doc.close()

    cv = Converter(str(work))
    try:
        cv.convert(str(dst), pages=pages, **settings)
    finally:
        cv.close()
        if tmp and tmp.exists():
            tmp.unlink()


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    extra = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
    convert(Path(sys.argv[1]), Path(sys.argv[2]), extra)
