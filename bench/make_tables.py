"""產生含表格的測試語料，並附上標準答案。

要回答的問題是「PDF → Excel 值不值得做」，而這完全取決於表格偵測的準確率。
所以語料必須帶 ground truth，準確率是算出來的，不是看起來像對的。

三種難度分開，因為它們的難度差距很大：
  bordered   有完整框線 —— 偵測器可以直接找線
  borderless 無框線，只靠對齊 —— 要從文字座標推斷欄界，這是真正難的部分
  merged     含合併儲存格 —— 連資料模型都對不上

用法: python bench/make_tables.py
產出: bench/corpus/tables_*.pdf 與 bench/corpus/tables_truth.json
"""
import json
import sys
from pathlib import Path

from reportlab.lib import colors
from reportlab.lib.pagesizes import A4
from reportlab.lib.styles import ParagraphStyle
from reportlab.platypus import Paragraph, SimpleDocTemplate, Spacer, Table, TableStyle

sys.path.insert(0, str(Path(__file__).parent))
from make_corpus import FONT_NAME, setup_font  # noqa: E402

OUT = Path(__file__).parent / "corpus"
OUT.mkdir(parents=True, exist_ok=True)
setup_font()

HEADER = ["項次", "品名", "數量", "單價", "小計"]
ROWS = [
    ["1", "辦公桌", "12", "3,200", "38,400"],
    ["2", "人體工學椅", "12", "5,800", "69,600"],
    ["3", "檔案櫃", "6", "2,150", "12,900"],
    ["4", "白板 120×90", "3", "1,480", "4,440"],
    ["5", "投影機吊架", "2", "990", "1,980"],
    ["6", "網路交換器", "4", "7,600", "30,400"],
    ["7", "無線基地台", "8", "2,880", "23,040"],
    ["8", "碎紙機", "1", "4,500", "4,500"],
]

BODY = ParagraphStyle("body", fontName=FONT_NAME, fontSize=10.5, leading=16)
TITLE = ParagraphStyle("title", fontName=FONT_NAME, fontSize=15, leading=22, spaceAfter=10)


def build(name: str, style: TableStyle, data: list[list[str]], note: str):
    path = OUT / f"tables_{name}.pdf"
    doc = SimpleDocTemplate(str(path), pagesize=A4, title=name)
    story = [
        Paragraph("設備採購清單", TITLE),
        Paragraph(note, BODY),
        Spacer(1, 14),
        Table(data, colWidths=[40, 150, 55, 70, 80], style=style),
        Spacer(1, 14),
        Paragraph("以上金額未稅。本文件為表格偵測測試語料，內容並無實質意義。", BODY),
    ]
    doc.build(story)
    return path


BASE = [
    ("FONTNAME", (0, 0), (-1, -1), FONT_NAME),
    ("FONTSIZE", (0, 0), (-1, -1), 10),
    ("ALIGN", (2, 0), (-1, -1), "RIGHT"),
    ("VALIGN", (0, 0), (-1, -1), "MIDDLE"),
    ("TOPPADDING", (0, 0), (-1, -1), 5),
    ("BOTTOMPADDING", (0, 0), (-1, -1), 5),
]


def main():
    data = [HEADER] + ROWS
    truth = {}

    build(
        "bordered",
        TableStyle(BASE + [
            ("GRID", (0, 0), (-1, -1), 0.6, colors.black),
            ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#dddddd")),
        ]),
        data,
        "以下表格有完整框線。",
    )
    truth["tables_bordered.pdf"] = {"rows": len(data), "cols": 5, "cells": data}

    build(
        "borderless",
        TableStyle(BASE + [("LINEBELOW", (0, 0), (-1, 0), 0.6, colors.black)]),
        data,
        "以下表格只有標題底線，欄位靠對齊區分。",
    )
    truth["tables_borderless.pdf"] = {"rows": len(data), "cols": 5, "cells": data}

    merged = [["設備採購清單（含合併儲存格）", "", "", "", ""]] + data
    build(
        "merged",
        TableStyle(BASE + [
            ("GRID", (0, 0), (-1, -1), 0.6, colors.black),
            ("SPAN", (0, 0), (-1, 0)),
            ("ALIGN", (0, 0), (-1, 0), "CENTER"),
        ]),
        merged,
        "以下表格第一列跨越五欄。",
    )
    truth["tables_merged.pdf"] = {"rows": len(merged), "cols": 5, "cells": merged}

    # 跨頁表格是真實文件最常見的失敗情境：偵測器容易把它切成兩個獨立表格，
    # 或是把每頁重複的標題列當成資料列。
    long_rows = [[str(i + 1), f"項目 {i + 1}", str((i * 7) % 50 + 1), "1,000", "1,000"]
                 for i in range(40)]
    long_data = [HEADER] + long_rows
    build(
        "spanning",
        TableStyle(BASE + [
            ("GRID", (0, 0), (-1, -1), 0.6, colors.black),
            ("BACKGROUND", (0, 0), (-1, 0), colors.HexColor("#dddddd")),
        ]),
        long_data,
        "以下表格共 40 列，會跨越兩頁。",
    )
    truth["tables_spanning.pdf"] = {"rows": len(long_data), "cols": 5, "cells": long_data}

    (OUT / "tables_truth.json").write_text(
        json.dumps(truth, ensure_ascii=False, indent=2), encoding="utf-8"
    )
    for name in truth:
        size = (OUT / name).stat().st_size / 1024
        print(f"{name:26} {size:6.1f} KB")
    print(f"{'tables_truth.json':26} 標準答案已寫入")


if __name__ == "__main__":
    main()
