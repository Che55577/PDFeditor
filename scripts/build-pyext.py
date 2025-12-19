"""建立 Python 擴充包。

主程式刻意不含 Python。PDF 轉 Word / Excel 與 OCR 這幾項只有 Python 生態有堪用的
實作（pdf2docx、OCRmyPDF），而它們的執行期加起來超過 200 MB——塞進主安裝檔會讓
體積從 25 MB 變十倍，但大多數人永遠用不到這些功能。

所以做成獨立的擴充包：要用的人才裝。它是一次性下載安裝，不是執行期連網取件，
「處理過程不連網」的宣稱不受影響。

用法: python scripts/build-pyext.py
產出: vendor/pyext/（可直接打包成擴充包安裝檔）
"""
import shutil
import subprocess
import sys
import urllib.request
import zipfile
from pathlib import Path

PY_VERSION = "3.11.9"
ROOT = Path(__file__).parent.parent
DEST = ROOT / "vendor" / "pyext"
CACHE = ROOT / "vendor" / "_cache"

EMBED_URL = (
    f"https://www.python.org/ftp/python/{PY_VERSION}/python-{PY_VERSION}-embed-amd64.zip"
)
GET_PIP = "https://bootstrap.pypa.io/get-pip.py"

# pdf2docx 預設會拉進完整的 opencv-python（112 MB）。headless 版少了 GUI 相關的
# 部分，功能上對我們完全等價，可以省下約 70 MB。
PACKAGES = ["pdf2docx"]
REPLACE_OPENCV = ("opencv-python", "opencv-python-headless")

# 這些在執行期用不到，但佔的空間很可觀
PRUNE_DIRS = {"__pycache__", "tests", "test", "testing", "examples", "docs"}
PRUNE_SUFFIXES = {".pyi", ".pdb", ".chm"}

# 裝完就用不到的安裝工具。擴充包是固定產物，不需要在使用者機器上自我更新。
#
# distutils-precedence.pth 一定要一起刪：它會在每次啟動時 import _distutils_hack，
# 而那個模組已經被刪掉了，結果是每次執行都印一段 ModuleNotFoundError。
PRUNE_PACKAGES = ["pip", "setuptools", "pkg_resources", "wheel", "_distutils_hack"]
PRUNE_FILES = ["Lib/site-packages/distutils-precedence.pth"]

# OpenCV 的影片編解碼器有 29 MB，而我們只做影像處理。實測移除後 pdf2docx
# 的匯入與轉檔都正常。
PRUNE_GLOBS = ["**/opencv_videoio_ffmpeg*.dll"]


def fetch(url: str, dest: Path) -> Path:
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.exists():
        print(f"  已快取 {dest.name}")
        return dest
    print(f"  下載 {url}")
    urllib.request.urlretrieve(url, dest)
    return dest


def run(*args: str) -> None:
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode != 0:
        tail = (result.stderr or result.stdout).strip().splitlines()[-6:]
        raise SystemExit("指令失敗：" + " ".join(args) + "\n" + "\n".join(tail))


def prune(root: Path) -> int:
    """刪掉執行期用不到的東西，回傳省下的位元組數。"""
    saved = 0

    site = root / "Lib" / "site-packages"
    for name in PRUNE_PACKAGES:
        for path in list(site.glob(name)) + list(site.glob(f"{name}-*")) + list(site.glob(f"{name}.py")):
            if path.is_dir():
                saved += sum(f.stat().st_size for f in path.rglob("*") if f.is_file())
                shutil.rmtree(path, ignore_errors=True)
            elif path.is_file():
                saved += path.stat().st_size
                path.unlink()

    for rel in PRUNE_FILES:
        path = root / rel
        if path.is_file():
            saved += path.stat().st_size
            path.unlink()

    for pattern in PRUNE_GLOBS:
        for path in root.glob(pattern):
            if path.is_file():
                saved += path.stat().st_size
                path.unlink()

    for path in sorted(root.rglob("*"), key=lambda p: -len(p.parts)):
        try:
            if path.is_dir() and path.name in PRUNE_DIRS:
                saved += sum(f.stat().st_size for f in path.rglob("*") if f.is_file())
                shutil.rmtree(path, ignore_errors=True)
            elif path.is_file() and path.suffix in PRUNE_SUFFIXES:
                saved += path.stat().st_size
                path.unlink()
        except OSError:
            pass
    return saved


def size_of(root: Path) -> float:
    return sum(f.stat().st_size for f in root.rglob("*") if f.is_file()) / 1048576


def main() -> None:
    if DEST.exists():
        print(f"清除既有的 {DEST}")
        shutil.rmtree(DEST)
    DEST.mkdir(parents=True)

    print("1. 取得可嵌入版 Python")
    zip_path = fetch(EMBED_URL, CACHE / f"python-{PY_VERSION}-embed.zip")
    with zipfile.ZipFile(zip_path) as z:
        z.extractall(DEST)

    print("2. 開啟 site-packages")
    # 可嵌入版預設把 import site 註解掉，不開啟就裝不了套件
    for pth in DEST.glob("python*._pth"):
        text = pth.read_text(encoding="utf-8")
        pth.write_text(text.replace("#import site", "import site"), encoding="utf-8")

    print("3. 安裝 pip")
    get_pip = fetch(GET_PIP, CACHE / "get-pip.py")
    python = DEST / "python.exe"
    run(str(python), str(get_pip), "--no-warn-script-location", "-q")

    print(f"4. 安裝 {', '.join(PACKAGES)}")
    run(str(python), "-m", "pip", "install", "-q", "--no-warn-script-location", *PACKAGES)

    print(f"5. 把 {REPLACE_OPENCV[0]} 換成 {REPLACE_OPENCV[1]}")
    run(str(python), "-m", "pip", "uninstall", "-y", "-q", REPLACE_OPENCV[0])
    run(str(python), "-m", "pip", "install", "-q", "--no-warn-script-location", REPLACE_OPENCV[1])

    before = size_of(DEST)
    print("6. 清除執行期用不到的檔案")
    saved = prune(DEST) / 1048576
    print(f"   省下 {saved:.1f} MB")

    print("7. 驗證")
    run(str(python), "-c", "import pdf2docx, fitz, cv2, docx; print('ok')")

    print(f"\n完成：{DEST}")
    print(f"  安裝後 {before:.1f} MB → 清理後 {size_of(DEST):.1f} MB")


if __name__ == "__main__":
    if sys.platform != "win32":
        raise SystemExit("目前只支援 Windows")
    main()
