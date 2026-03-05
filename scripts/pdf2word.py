"""PDF 轉 Word。主程式與 bench 共用這一支，避免兩邊行為飄移。

用法: python scripts/pdf2word.py <來源.pdf> <輸出.docx> [設定 JSON]

## 為什麼需要「向量前處理」

pdf2docx 的資料模型只有三種東西：文字、點陣圖、以及能變成表格框或底色的簡單形狀。
**任意向量圖形無處可去**——條碼、QR 碼、向量商標這類東西會直接消失。

所以在交給 pdf2docx 之前，先把向量密集的區域算成點陣圖疊回原處。pdf2docx 看到的
就是一張普通圖片，自然會保留。

難的不是點陣化，是**分辨哪些該點陣化**——條碼、商標、表格框線在 PDF 裡都只是向量路徑。
用兩個訊號，各自負責一種圖形（實測數字見 `MIN_PATH_DENSITY` 與 `_curve_count`）：

* **密度**——條碼與 QR 是上百條細線擠在一小塊，表格框線是少數幾條攤在一大片。
  （一開始用路徑「數量」，錯得很徹底：有框線的表格光框線就 17 條、跨頁表格 36 條，
  全都超過數量門檻，整張表會被轉成一張圖。）
* **曲線**——商標稀疏到密度和框線一樣低，但框線只有直線與矩形，商標幾乎一定有貝茲曲線。

圓角表格的框線也有曲線，會誤觸第二個訊號，所以那條路徑要再問一次 `find_tables`。
密集的群集不必問——表格不可能那麼密，而條碼很可能就畫在儲存格裡。
"""
import json
import logging
import sys
from pathlib import Path

# pdf2docx 每頁都會印一行進度到 stderr，一份幾百頁的文件會塞爆管線緩衝區。
logging.disable(logging.INFO)

# 一個群集至少要有幾條路徑才值得看。低於這個數的一定是版面線條。
MIN_PATHS_PER_CLUSTER = 8
# 路徑密度門檻（每 pt²）。**這是分開「圖形」與「表格框線」的關鍵。**
# 實測（bench/corpus，每 1000pt²）：QR 碼 36.6、Code128 條碼 7.3，
# 而有框線表格 0.22、合併儲存格 0.20、跨頁表格 0.15、細線表格 0.10。
# 兩群差 33 倍，門檻取中間的 1.0，兩邊各有 7 倍與 4.5 倍餘裕。
MIN_PATH_DENSITY = 1.0 / 1000
# 群集之間的間距門檻（pt）。條碼的線條間距遠小於此。
CLUSTER_GAP = 12.0
# 點陣化的解析度。條碼要夠清晰才掃得動。
RASTER_DPI = 300
# 太大的群集不處理——那多半是整頁的框線，不是圖形。
MAX_CLUSTER_FRACTION = 0.55
# 清除原路徑時把範圍往外撐一點（pt）。群集 bbox 是成員矩形的聯集，邊界必然和最外側的
# 路徑完全重合，而「被覆蓋才移除」在邊界重合時判定不成立——實測商標的橢圓因此沒被清掉，
# pdf2docx 又自己把它算成第二張圖疊上來，圓點就不見了。撐 1pt 就避開這個等號。
REDACT_PAD = 1.0


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


def _curve_count(drawings, rect) -> int:
    """群集裡有幾段貝茲曲線。

    這是認出稀疏向量商標的唯一辦法。實測 bench/corpus：商標 7 段曲線，
    而細線表格、有框線表格、合併儲存格、跨頁表格的曲線數**全部是 0**。
    群集的 bbox 依定義涵蓋所有成員，所以用 contains 就找得回來。
    """
    return sum(
        1
        for d in drawings
        if rect.contains(d["rect"])
        for it in d["items"]
        if it[0] == "c"
    )


def rasterize_vector_art(doc, dpi=RASTER_DPI) -> int:
    """把該保住的向量圖形算成點陣圖疊回原處。回傳處理了幾塊。"""
    import pymupdf

    done = 0
    for page in doc:
        drawings = page.get_drawings()
        if not drawings:
            continue
        page_area = abs(page.rect.width * page.rect.height)
        groups = cluster([d["rect"] for d in drawings])
        tables = None  # 延後計算：find_tables 一頁約 20ms，多數群集用不到

        targets = []
        for box, count in groups:
            rect = pymupdf.Rect(box)
            if rect.width < 2 or rect.height < 2:
                continue
            area = abs(rect.width * rect.height)
            if area > page_area * MAX_CLUSTER_FRACTION:
                continue  # 太大，多半是整頁的框線

            dense = count >= MIN_PATHS_PER_CLUSTER and count / area >= MIN_PATH_DENSITY
            if not dense:
                # 稀疏的群集：全是直線就是框線，有曲線才可能是商標這類圖形。
                if not _curve_count(drawings, rect):
                    continue
                # 圓角表格的框線也有曲線，只有這條路徑會誤觸，所以只有這裡要問。
                # 密集的不問——表格不可能那麼密，而條碼很可能就畫在儲存格裡。
                if tables is None:
                    tables = [pymupdf.Rect(t.bbox) for t in page.find_tables()]
                if any((t & rect).get_area() > 0.5 * area for t in tables):
                    continue
            targets.append(rect)

        if not targets:
            continue

        # 算圖要在清除之前，否則清完就沒東西可算了。
        pixmaps = [(r, page.get_pixmap(clip=r, dpi=dpi)) for r in targets]

        # **只疊圖是不夠的**：原本的向量路徑還留在檔案裡，pdf2docx 照樣會處理它們，
        # 實測一個 QR 碼會被打碎成 13 張碎圖塞進 docx。所以要把蓋住的路徑真的移除。
        # text=NONE 保住文字（塗黑用的 redaction 預設會連文字一起刪），images=NONE 不動既有圖片。
        pad = (-REDACT_PAD, -REDACT_PAD, REDACT_PAD, REDACT_PAD)
        for rect in targets:
            page.add_redact_annot(rect + pad, fill=None, cross_out=False)
        page.apply_redactions(
            images=pymupdf.PDF_REDACT_IMAGE_NONE,
            graphics=pymupdf.PDF_REDACT_LINE_ART_REMOVE_IF_COVERED,
            text=pymupdf.PDF_REDACT_TEXT_NONE,
        )

        for rect, pix in pixmaps:
            page.insert_image(rect, pixmap=pix, overlay=True)
            done += 1
    return done


def convert(src: Path, dst: Path, settings: dict) -> int:
    """轉檔，回傳點陣化了幾塊向量圖形（給 bench 用來分辨「沒點陣化」和「點陣化了但仍遺失」）。"""
    import pymupdf
    from pdf2docx import Converter

    keep_vector = settings.pop("keep_vector_art", True)
    pages = settings.pop("pages", None)

    work = src
    # 中間檔開在輸出檔旁邊。正常會自己刪掉，但硬砍行程時不會——
    # **所以呼叫端的輸出路徑必須在暫存區內**，才有啟動掃除當保險。見 src-tauri/src/ephemeral.rs。
    tmp = None
    rasterized = 0
    if keep_vector:
        doc = pymupdf.open(src)
        try:
            rasterized = rasterize_vector_art(doc)
            if rasterized:
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
    return rasterized


if __name__ == "__main__":
    if len(sys.argv) < 3:
        sys.exit(__doc__)
    extra = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
    print(convert(Path(sys.argv[1]), Path(sys.argv[2]), extra))
