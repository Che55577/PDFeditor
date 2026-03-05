"""量表格偵測準確率，用來決定 PDF → Excel 值不值得做。

同時是**向量點陣化的防呆測試**：`scripts/pdf2word.py` 會把向量密集的區域轉成圖片，
表格框線一旦被誤判成圖形，整張表就會變成一張圖、文字全部拿不回來。所以這裡刻意
走產品同一支腳本，並印出點陣化了幾塊——**表格語料這一欄必須是 0**。

需要裝了 pdf2docx 的環境（見 docs/pdf-to-office.md）。
用法: <pyext>/python.exe bench/table_bench.py
"""
import json
import logging
import sys
import tempfile
import time
from pathlib import Path

logging.disable(logging.INFO)

sys.path.insert(0, str(Path(__file__).parent.parent / "scripts"))

from docx import Document  # noqa: E402
from pdf2word import convert  # noqa: E402

CORPUS = Path(__file__).parent / "corpus"
TRUTH = json.loads((CORPUS / "tables_truth.json").read_text(encoding="utf-8"))


def extract_tables(docx_path: Path) -> list[list[list[str]]]:
    """回傳每個表格的儲存格文字矩陣。"""
    out = []
    for table in Document(str(docx_path)).tables:
        out.append([[cell.text.strip() for cell in row.cells] for row in table.rows])
    return out


def norm(s: str) -> str:
    """比對前正規化：去掉空白與全形空格，數字千分位不計較。"""
    return s.replace(" ", "").replace("　", "").replace(",", "").strip()


def score(expected: list[list[str]], got: list[list[str]]) -> tuple[int, int]:
    """回傳 (對的儲存格數, 應有的儲存格數)。"""
    total = sum(len(r) for r in expected)
    hit = 0
    for r, row in enumerate(expected):
        for c, want in enumerate(row):
            if not want:
                total -= 1  # 合併後的空儲存格不計分
                continue
            if r < len(got) and c < len(got[r]) and norm(got[r][c]) == norm(want):
                hit += 1
    return hit, total


def stitch(tables: list[list[list[str]]]) -> list[list[str]]:
    """把欄數相同的連續表格接成一個。

    跨頁表格會被 pdf2docx 拆成每頁一個，這是很自然的行為——它看到的就是兩塊。
    要還原成一張表，把欄數一致的相鄰表格接起來，並丟掉重複出現的標題列。
    """
    if not tables:
        return []
    merged = [row[:] for row in tables[0]]
    header = merged[0] if merged else []
    for table in tables[1:]:
        if not table or len(table[0]) != len(header):
            continue
        rows = table
        # 每頁重複的標題列不該算成資料
        if [norm(c) for c in rows[0]] == [norm(c) for c in header]:
            rows = rows[1:]
        merged.extend(row[:] for row in rows)
    return merged


def main():
    print(f"{'語料':22} {'秒':>5} {'點陣化':>6} {'表格數':>6} {'列×欄':>9} {'':>12} {'原始':>6} {'接合後':>8}")
    print("-" * 82)
    for name, truth in TRUTH.items():
        src = CORPUS / name
        if not src.exists():
            print(f"{name:22} 找不到語料，請先跑 make_tables.py")
            continue

        with tempfile.TemporaryDirectory() as tmp:
            dst = Path(tmp) / "out.docx"
            t0 = time.perf_counter()
            rasterized = convert(src, dst, {})
            elapsed = time.perf_counter() - t0
            tables = extract_tables(dst)

        if not tables:
            print(f"{name:22} {elapsed:5.1f} {rasterized:>6} {0:>6} {'—':>10} {'完全沒偵測到':>12}")
            continue

        # 取最大的那個表格——這是「不做任何後處理」的基準。
        biggest = max(tables, key=lambda t: sum(len(r) for r in t))
        hit, total = score(truth["cells"], biggest)

        # 跨頁表格會被切成多個，接回去看看能不能還原。
        stitched = stitch(tables)
        s_hit, s_total = score(truth["cells"], stitched)

        shape = f"{len(biggest)}×{max((len(r) for r in biggest), default=0)}"
        want = f"{truth['rows']}×{truth['cols']}"
        mark = "" if shape == want else f"(應為 {want})"
        print(
            f"{name:22} {elapsed:5.1f} {rasterized:>6} {len(tables):>6} {shape:>9}{mark:>12} "
            f"{hit / total:>6.0%} {s_hit / s_total:>8.0%}"
        )


if __name__ == "__main__":
    if not (CORPUS / "tables_truth.json").exists():
        sys.exit("請先跑 python bench/make_tables.py")
    main()
