#!/usr/bin/env bash
# 在 macOS 上從原始碼編譯 Ghostscript，產出 vendor/ghostscript/bin/gs。
#
# Artifex 不提供 macOS 二進位檔；Homebrew 版依賴一串 dylib（libtiff、openjpeg、
# fontconfig…），不能直接塞進 App。原始碼包已附帶 jpeg/png/zlib/freetype/lcms2/
# openjpeg/jbig2dec/tiff，用它們靜態連結，產出只依賴系統函式庫的單一執行檔。
#
# 預設以 --enable-compile-inits 編譯：lib/、Resource/、iccprofiles/ 全部內建在執行檔裡，
# 和 Windows 官方版一樣不需要旁邊附資料夾。
#
# 用法（通常由 scripts/fetch-vendor.mjs 呼叫）:
#   bash scripts/build-gs-macos.sh                 # 本機架構
#   ARCH=x86_64 bash scripts/build-gs-macos.sh     # 指定架構（Apple Silicon 上需要 Rosetta）
#   ARCH=universal bash scripts/build-gs-macos.sh  # arm64 + x86_64 合成一個
#
# 需要：Xcode Command Line Tools（xcode-select --install）。
set -euo pipefail

GS_VERSION="${GS_VERSION:-10.07.1}"
GS_TAG="${GS_TAG:-gs10071}"
GS_SHA256="2fc74362f9be6fae1b0a65d38fdcfd4f0b518cc3b07c5581fb661eb4d2e15251"
ARCH="${ARCH:-$(uname -m)}"

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CACHE="$ROOT/vendor/_cache"
DEST="$ROOT/vendor/ghostscript/bin"
TARBALL="$CACHE/ghostscript-$GS_VERSION.tar.gz"
URL="https://github.com/ArtifexSoftware/ghostpdl-downloads/releases/download/$GS_TAG/ghostscript-$GS_VERSION.tar.gz"

[[ "$(uname -s)" == "Darwin" ]] || { echo "只能在 macOS 上執行" >&2; exit 1; }

mkdir -p "$CACHE"
if [[ -f "$TARBALL" ]]; then
  actual="$(shasum -a 256 "$TARBALL" | awk '{print $1}')"
  if [[ "$actual" != "$GS_SHA256" ]]; then
    echo "[gs] 快取 SHA-256 不符，重新下載"
    rm -f "$TARBALL"
  fi
fi
if [[ ! -f "$TARBALL" ]]; then
  echo "[gs] 下載 $URL"
  curl -fL --retry 3 -o "$TARBALL.part" "$URL"
  actual="$(shasum -a 256 "$TARBALL.part" | awk '{print $1}')"
  [[ "$actual" == "$GS_SHA256" ]] || { echo "[gs] SHA-256 校驗失敗：$actual" >&2; rm -f "$TARBALL.part"; exit 1; }
  mv "$TARBALL.part" "$TARBALL"
fi

build_one() {
  local arch="$1"
  local work="$CACHE/gs-build-$arch"
  rm -rf "$work"
  mkdir -p "$work"
  tar -xzf "$TARBALL" -C "$work" --strip-components=1

  echo "[gs] 編譯 $arch"
  (
    cd "$work"
    # 徹底隔離 Homebrew：Ghostscript 的 configure.ac 檢查的是 PKGCONFIG（無底線）而非 PKG_CONFIG！
    # 同時將 PATH 限制為純系統目錄，防止 configure 撿到 /opt/homebrew 下的動態庫。
    export PATH="/usr/bin:/bin:/usr/sbin:/sbin"
    export PKGCONFIG=/usr/bin/false
    export PKG_CONFIG=/usr/bin/false
    export CC="clang -arch $arch"
    export CFLAGS="-O2 -mmacosx-version-min=11.0 -Wno-implicit-function-declaration -Wno-int-conversion -Wno-incompatible-function-pointer-types"
    export LDFLAGS="-mmacosx-version-min=11.0"
    ./configure \
      --disable-cups --disable-dbus --disable-gtk --disable-fontconfig --disable-contrib \
      --with-libiconv=no --with-local-zlib --with-drivers=FILES \
      --without-x --without-libidn --without-libpaper --without-tesseract \
      --without-ijs --without-pdftoraster --without-cal --without-so \
      --enable-compile-inits \
      > "$work/configure.log" 2>&1 || { tail -50 "$work/configure.log"; exit 1; }
    make -j"$(sysctl -n hw.ncpu)" > "$work/make.log" 2>&1 || { tail -50 "$work/make.log"; exit 1; }
  )
  strip -x "$work/bin/gs"
  if [[ -f "$work/LICENSE" ]]; then
    mkdir -p "$ROOT/vendor/ghostscript"
    cp "$work/LICENSE" "$ROOT/vendor/ghostscript/LICENSE"
  fi
}

case "$ARCH" in
  universal)
    build_one arm64
    build_one x86_64
    mkdir -p "$DEST"
    lipo -create "$CACHE/gs-build-arm64/bin/gs" "$CACHE/gs-build-x86_64/bin/gs" -output "$DEST/gs"
    ;;
  arm64|x86_64)
    build_one "$ARCH"
    mkdir -p "$DEST"
    cp "$CACHE/gs-build-$ARCH/bin/gs" "$DEST/gs"
    ;;
  *)
    echo "不支援的 ARCH：$ARCH（可用 arm64、x86_64、universal）" >&2
    exit 1
    ;;
esac

chmod 755 "$DEST/gs"
# strip / lipo 會讓簽章失效；Apple Silicon 拒絕執行沒簽章的程式，先補 ad-hoc 簽章。
# 正式發佈時 scripts/sign-macos.sh 會再用 Developer ID 重簽。
codesign --force --sign - "$DEST/gs"

# 只允許連結系統函式庫，否則換一台 Mac 就跑不起來。
# （只看縮排的行；universal 版的輸出每個架構會多一行不縮排的標題）
bad="$(otool -L "$DEST/gs" | grep -E '^[[:space:]]' | awk '{print $1}' | grep -vE '^(/usr/lib/|/System/Library/)' || true)"
if [[ -n "$bad" ]]; then
  echo "[gs] 錯誤：連結到非系統函式庫，無法隨 App 散布：" >&2
  echo "$bad" >&2
  exit 1
fi

ver="$("$DEST/gs" --version)"
[[ "$ver" == "$GS_VERSION" ]] || { echo "[gs] 版本不符：${ver}" >&2; exit 1; }
echo "[gs] 完成：${DEST}/gs（${ver}，$(du -h "$DEST/gs" | cut -f1)，$(lipo -archs "$DEST/gs")）"
