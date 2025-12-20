"""找出讓 pdf2docx 保住條碼等向量圖形的設定。

條碼在 PDF 裡不是點陣圖，是一堆極細的填色矩形。pdf2docx 預設會把太細的形狀
當成雜訊丟掉（`shape_min_dimension`），也只在向量路徑佔比夠高時才把整塊區域
點陣化（`curve_path_ratio`）——結果就是使用者說的「條碼不見了」。

這支程式對同一份語料試不同設定，量兩件事：
* 輸出的 docx 裡有幾張圖片（條碼若被點陣化保留，會是一張圖）
* 來回轉回 PDF 後與原稿的 SSIM

用系統 Python 執行，pdf2docx 那一步以子行程呼叫擴充包。
用法: python bench/vector_loss.py
"""
import json
import shutil
import subprocess
import sys
import tempfile
import zipfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
from word_fidelity import compare, ghostscript, pyext_python, soffice  # noqa: E402

ROOT = Path(__file__).parent.parent
CORPUS = ROOT / "bench" / "corpus"
PAGES = 1

# pdf2docx 的版面設定。鍵名要用真的存在的——`Converter.default_settings` 才是準的，
# 傳不存在的鍵會被安靜忽略（我第一次就傳了不存在的 curve_path_ratio，白測一輪）。
PROFILES: dict[str, dict] = {
    "預設": {},
    "細形狀不丟": {"shape_min_dimension": 0.1},
    "向量群組放寬": {"min_svg_w": 0.2, "min_svg_h": 0.2, "min_svg_gap_dx": 2.0, "min_svg_gap_dy": 0.5},
    "關掉表格解析": {"parse_lattice_table": False, "parse_stream_table": False},
    "關表格＋細形狀": {
        "parse_lattice_table": False,
        "parse_stream_table": False,
        "shape_min_dimension": 0.1,
    },
    "全開": {
        "parse_lattice_table": False,
        "parse_stream_table": False,
        "shape_min_dimension": 0.1,
        "min_svg_w": 0.2,
        "min_svg_h": 0.2,
        "min_svg_gap_dx": 2.0,
        "min_svg_gap_dy": 0.5,
        "clip_image_res_ratio": 5.0,
    },
}

CONVERT = """
import sys, json, logging
logging.disable(logging.INFO)
from pdf2docx import Converter
settings = json.loads(sys.argv[4])
cv = Converter(sys.argv[1])
try:
    cv.convert(sys.argv[2], pages=list(range(int(sys.argv[3]))), **settings)
finally:
    cv.close()
"""


def to_docx(py: str, src: Path, dst: Path, pages: int, settings: dict) -> None:
    result = subprocess.run(
        [py, "-c", CONVERT, str(src), str(dst), str(pages), json.dumps(settings)],
        capture_output=True, text=True,
    )
    if result.returncode != 0 or not dst.exists():
        raise RuntimeError((result.stderr or "轉檔失敗").strip()[-300:])


def count_images(docx: Path) -> int:
    with zipfile.ZipFile(docx) as z:
        return sum(1 for n in z.namelist() if n.startswith("word/media/"))


def to_pdf(so: str, docx: Path, work: Path) -> Path:
    subprocess.run(
        [so, f"-env:UserInstallation=file:///{work.as_posix()}/profile",
         "--headless", "--norestore", "--convert-to", "pdf", "--outdir", str(work), str(docx)],
        check=True, capture_output=True)
    return work / (docx.stem + ".pdf")


def main(src: Path):
    gs, so, py = ghostscript(), soffice(), pyext_python()
    if not gs or not so or not py:
        sys.exit("需要 vendor/ghostscript、vendor/pyext 與 LibreOffice")
    if not src.exists():
        sys.exit(f"找不到 {src}，請先跑 python bench/make_corpus.py")

    print(f"=== {src.name} ===")
    print(f"{'設定':22} {'圖片數':>6} {'SSIM':>8}")
    print("-" * 40)
    for label, settings in PROFILES.items():
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            docx = work / "out.docx"
            try:
                to_docx(py, src, docx, PAGES, settings)
            except RuntimeError as e:
                print(f"{label:22} 失敗：{e}")
                continue
            images = count_images(docx)
            back = to_pdf(so, docx, work)
            scores = compare(gs, src, back, work)
            shutil.rmtree(work / "ref", ignore_errors=True)
            shutil.rmtree(work / "got", ignore_errors=True)
            avg = sum(scores) / len(scores) if scores else float("nan")
            print(f"{label:22} {images:>6} {avg:>8.4f}")


if __name__ == "__main__":
    main(Path(sys.argv[1]) if len(sys.argv) > 1 else CORPUS / "c4_vector.pdf")
