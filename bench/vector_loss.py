"""量條碼等向量圖形在 PDF → Word 之後保住了沒有。

條碼在 PDF 裡不是點陣圖，是一堆極細的填色矩形。**pdf2docx 沒有任何設定救得回來**——
它的資料模型只有文字、點陣圖、以及能變成表格框或底色的簡單形狀，任意向量圖形無處可去。
第一輪掃過各種設定（`shape_min_dimension`、`min_svg_*`、`clip_image_res_ratio`）都是
輸出圖片數 0；唯一有效的是關掉表格解析，SSIM 0.892 → 0.941，但條碼一樣沒了。

所以真正的修法在 `scripts/pdf2word.py`：交給 pdf2docx 之前先把向量密集的區域點陣化。
這支程式量的就是那個前處理的效果——刻意呼叫和產品同一支腳本，避免兩邊行為飄移。

量兩件事：
* 輸出的 docx 裡有幾張圖片（條碼若被點陣化保留，會是一張圖）
* 來回轉回 PDF 後與原稿的 SSIM

用系統 Python 執行，轉檔那一步以子行程呼叫擴充包。
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
SCRIPT = ROOT / "scripts" / "pdf2word.py"
PAGES = 1

# 關掉表格解析：在這份語料上單獨就有 +0.05 SSIM，因為細線表格本來會被誤判。
# 鍵名要用真的存在的——`Converter.default_settings` 才是準的，
# 傳不存在的鍵會被安靜忽略（我第一次就傳了不存在的 curve_path_ratio，白測一輪）。
NO_TABLE = {"parse_lattice_table": False, "parse_stream_table": False}

PROFILES: dict[str, dict] = {
    "原樣（不點陣化）": {"keep_vector_art": False},
    "只關表格解析": {"keep_vector_art": False, **NO_TABLE},
    "點陣化向量圖形": {},
    "點陣化＋關表格": dict(NO_TABLE),
}


def to_docx(py: str, src: Path, dst: Path, pages: int, settings: dict) -> int:
    """回傳腳本點陣化了幾塊。"""
    settings = {"pages": list(range(pages)), **settings}
    result = subprocess.run(
        [py, str(SCRIPT), str(src), str(dst), json.dumps(settings)],
        capture_output=True, text=True,
    )
    if result.returncode != 0 or not dst.exists():
        raise RuntimeError((result.stderr or "轉檔失敗").strip()[-300:])
    # pdf2docx 匯入 fitz 會在 stdout 印棄用警告，數字在最後一行
    return int((result.stdout.strip() or "0").splitlines()[-1])


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
    print(f"{'設定':18} {'點陣化':>6} {'圖片數':>6} {'SSIM':>8}")
    print("-" * 46)
    for label, settings in PROFILES.items():
        with tempfile.TemporaryDirectory() as tmp:
            work = Path(tmp)
            docx = work / "out.docx"
            try:
                blocks = to_docx(py, src, docx, PAGES, dict(settings))
            except RuntimeError as e:
                print(f"{label:18} 失敗：{e}")
                continue
            images = count_images(docx)
            back = to_pdf(so, docx, work)
            scores = compare(gs, src, back, work)
            shutil.rmtree(work / "ref", ignore_errors=True)
            shutil.rmtree(work / "got", ignore_errors=True)
            avg = sum(scores) / len(scores) if scores else float("nan")
            print(f"{label:18} {blocks:>6} {images:>6} {avg:>8.4f}")


if __name__ == "__main__":
    main(Path(sys.argv[1]) if len(sys.argv) > 1 else CORPUS / "c4_vector.pdf")
