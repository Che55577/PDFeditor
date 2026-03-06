"""建立 Python 擴充包（Windows / macOS 共用）。

主程式刻意不含 Python。PDF 轉 Word / Excel 與 OCR 這幾項只有 Python 生態有堪用的
實作（pdf2docx、OCRmyPDF），而它們的執行期加起來約 200 MB——塞進主安裝檔會讓
體積從 25 MB 變十倍，但大多數人永遠用不到這些功能。

所以做成獨立的擴充包：要用的人才裝。它是一次性下載安裝，不是執行期連網取件，
「處理過程不連網」的宣稱不受影響。

Windows 與 macOS 統一使用 Astral 維護的 python-build-standalone（可搬移、不綁本機路徑、
已內建 pip），因此兩個平台走同一套流程：
  - Windows 產出：vendor/pyext/python.exe
  - macOS 產出：  vendor/pyext/bin/python3

用法:
  python scripts/build-pyext.py
  LOCALPDF_ARCH=x86_64 python scripts/build-pyext.py   # macOS 上跨架構建立時可指定
產出: vendor/pyext/（可直接打包成擴充包安裝檔）
"""
import os
import platform
import shutil
import subprocess
import sys
import tarfile
import urllib.request
from pathlib import Path

PBS_TAG = "20261021"
PY_VERSION = "3.11.17"
ROOT = Path(__file__).parent.parent
DEST = ROOT / "vendor" / "pyext"
CACHE = ROOT / "vendor" / "_cache"

TARGETS = {
    ("win32", "amd64"): "x86_64-pc-windows-msvc",
    ("win32", "x86_64"): "x86_64-pc-windows-msvc",
    ("win32", "arm64"): "aarch64-pc-windows-msvc",
    ("darwin", "arm64"): "aarch64-apple-darwin",
    ("darwin", "aarch64"): "aarch64-apple-darwin",
    ("darwin", "x86_64"): "x86_64-apple-darwin",
}

# pdf2docx 預設會拉進完整的 opencv-python（112 MB）。headless 版少了 GUI 相關的
# 部分，功能上對我們完全等價，可以省下約 70 MB。
PACKAGES = ["pdf2docx"]
REPLACE_OPENCV = ("opencv-python", "opencv-python-headless")

# 這些在執行期用不到，但佔的空間很可觀
PRUNE_DIRS = {
    "__pycache__", "tests", "test", "testing", "examples", "docs",
    "idlelib", "tkinter", "turtledemo", "ensurepip", "venv", "pydoc_data", "lib2to3",
}
PRUNE_SUFFIXES = {".pyi", ".pdb", ".chm", ".a"}

# python-build-standalone 附帶 C 標頭、靜態連結庫與 Tcl/Tk，執行期完全不需要
PRUNE_TOP_DIRS = ["include", "libs", "tcl", "share", "Scripts"]

# 裝完就用不到的安裝工具。擴充包是固定產物，不需要在使用者機器上自我更新。
#
# distutils-precedence.pth 一定要一起刪：它會在每次啟動時 import _distutils_hack，
# 而那個模組已經被刪掉了，結果是每次執行都印一段 ModuleNotFoundError。
PRUNE_PACKAGES = ["pip", "setuptools", "pkg_resources", "wheel", "_distutils_hack"]

# OpenCV 的影片編解碼器有 29 MB，而我們只做影像處理。實測移除後 pdf2docx
# 的匯入與轉檔都正常。連同 Tcl/Tk 與 CPython 自測模組一併清除。
PRUNE_GLOBS = [
    "**/opencv_videoio_ffmpeg*",
    "**/libtcl*",
    "**/libtk*",
    "**/tcl8*",
    "**/tk8*",
    "**/_tkinter*",
    "**/_test*",
    "**/_ctypes_test*",
    "lib/tcl*",
    "lib/tk*",
    "lib/itcl*",
    "lib/tdbc*",
    "lib/thread*",
    "lib/pkgconfig",
    "lib/python*/config-*",
]


def resolve_target() -> str:
    arch = (os.environ.get("LOCALPDF_ARCH") or platform.machine()).lower()
    triple = TARGETS.get((sys.platform, arch))
    if not triple:
        raise SystemExit(f"不支援的平台組合：{sys.platform} / {arch}")
    return triple


def python_bin(root: Path) -> Path:
    return root / "python.exe" if sys.platform == "win32" else root / "bin" / "python3"


def site_packages(root: Path) -> Path:
    if sys.platform == "win32":
        return root / "Lib" / "site-packages"
    return next((root / "lib").glob("python3.*/site-packages"))


def fetch(url: str, dest: Path) -> Path:
    dest.parent.mkdir(parents=True, exist_ok=True)
    if dest.exists():
        print(f"  已快取 {dest.name}")
        return dest
    print(f"  下載 {url}")
    part = dest.with_suffix(dest.suffix + ".part")
    urllib.request.urlretrieve(url, part)
    part.replace(dest)
    return dest


def run(*args: str) -> None:
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode != 0:
        tail = (result.stderr or result.stdout).strip().splitlines()[-6:]
        raise SystemExit("指令失敗：" + " ".join(args) + "\n" + "\n".join(tail))


def remove_path(path: Path) -> int:
    if path.is_symlink() or path.is_file():
        size = 0 if path.is_symlink() else path.stat().st_size
        path.unlink(missing_ok=True)
        return size
    if path.is_dir():
        size = sum(f.stat().st_size for f in path.rglob("*") if f.is_file() and not f.is_symlink())
        shutil.rmtree(path, ignore_errors=True)
        return size
    return 0


def prune(root: Path) -> int:
    """刪掉執行期用不到的東西，回傳省下的位元組數。"""
    saved = 0

    for rel in PRUNE_TOP_DIRS:
        saved += remove_path(root / rel)

    site = site_packages(root)
    for name in PRUNE_PACKAGES:
        for path in list(site.glob(name)) + list(site.glob(f"{name}-*")) + list(site.glob(f"{name}.py")):
            saved += remove_path(path)
    saved += remove_path(site / "distutils-precedence.pth")

    for pattern in PRUNE_GLOBS:
        for path in list(root.glob(pattern)):
            saved += remove_path(path)

    for path in sorted(root.rglob("*"), key=lambda p: -len(p.parts)):
        try:
            if path.is_dir() and path.name in PRUNE_DIRS:
                saved += remove_path(path)
            elif path.is_file() and not path.is_symlink() and path.suffix in PRUNE_SUFFIXES:
                saved += remove_path(path)
        except OSError:
            pass
    return saved


def size_of(root: Path) -> float:
    return sum(
        f.stat().st_size for f in root.rglob("*") if f.is_file() and not f.is_symlink()
    ) / 1048576


def extract_pbs(archive: Path, dest: Path) -> None:
    """解開 python-build-standalone，剝掉頂層的 python/ 目錄。"""
    tmp = dest.parent / "_pyext_extract"
    if tmp.exists():
        shutil.rmtree(tmp)
    with tarfile.open(archive, "r:gz") as tar:
        tar.extractall(tmp, filter="data")
    inner = tmp / "python"
    inner.replace(dest)
    shutil.rmtree(tmp, ignore_errors=True)


def main() -> None:
    triple = resolve_target()
    archive_name = f"cpython-{PY_VERSION}+{PBS_TAG}-{triple}-install_only_stripped.tar.gz"
    url = f"https://github.com/astral-sh/python-build-standalone/releases/download/{PBS_TAG}/{archive_name}"

    if DEST.exists():
        print(f"清除既有的 {DEST}")
        shutil.rmtree(DEST)
    DEST.parent.mkdir(parents=True, exist_ok=True)

    print(f"1. 取得可搬移版 Python（{triple}）")
    archive = fetch(url, CACHE / archive_name)
    extract_pbs(archive, DEST)

    python = python_bin(DEST)
    print(f"2. 安裝 {', '.join(PACKAGES)}")
    run(str(python), "-m", "pip", "install", "-q", "--no-warn-script-location", *PACKAGES)

    print(f"3. 把 {REPLACE_OPENCV[0]} 換成 {REPLACE_OPENCV[1]}")
    run(str(python), "-m", "pip", "uninstall", "-y", "-q", REPLACE_OPENCV[0])
    run(str(python), "-m", "pip", "install", "-q", "--no-warn-script-location", REPLACE_OPENCV[1])

    before = size_of(DEST)
    print("4. 清除執行期用不到的檔案")
    saved = prune(DEST) / 1048576
    print(f"   省下 {saved:.1f} MB")

    print("5. 驗證")
    run(str(python), "-c", "import pdf2docx, fitz, cv2, docx; print('ok')")

    print(f"\n完成：{DEST}")
    print(f"  安裝後 {before:.1f} MB → 清理後 {size_of(DEST):.1f} MB")


if __name__ == "__main__":
    main()
