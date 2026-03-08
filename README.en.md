# LocalPDF

[![CI](https://github.com/Che55577/PDFeditor/actions/workflows/ci.yml/badge.svg)](https://github.com/Che55577/PDFeditor/actions/workflows/ci.yml)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)
![Platform: Windows | macOS | Linux](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)

[English](README.en.md) | [繁體中文](README.md)

A privacy-first, local offline desktop toolbox for PDF files. Your documents never leave your machine; all intermediate temporary files are zero-filled and deleted immediately upon task completion or window closure.

Currently supports: **Compress**, **Merge**, **Extract Pages**, **Remove Pages**, **Rotate**, **Crop**, **Split**, **Convert to Images** (JPG/PNG), **Images to PDF**, **Repair**, **Watermark**, **Page Numbers**, **Stamp Overlay**, **Office to PDF**, **Convert to Markdown**, and an interactive **Annotation & Redaction Editor**.

**PDF to Word** requires installing an optional standalone Python extension package (detailed below). We intentionally do not offer PDF to PowerPoint conversion, as no open-source engine currently produces satisfactory layout quality.

---

### Need to Edit PDF Text In-Place?

**You cannot.** A PDF does not contain structured paragraphs; its content streams consist of positioned glyph runs with absolute coordinates. Modifying a single character would require re-measuring glyph widths, reflowing subsequent words, and recalculating line wraps—structural context that was permanently discarded when the original PDF was compiled. LocalPDF's editor provides **layered annotations and true redaction**, rather than destructive in-place text mutation.

The only reliable workflow is: **PDF → Word (.docx) → Edit inside Word (or LibreOffice Writer) → Convert back to PDF via "Office to PDF"**. LocalPDF handles both ends of this pipeline.

### Font Remapping for PDF to Word

`pdf2docx` writes PDF font names directly into Word `.docx` documents. However, its normalization frequently outputs font family names that **do not exist on the host OS**:
For instance, a source subset `AAAAAA+DFKaiShu-SB-Estd-BF` gets written as `DFKai`. Because Windows lacks a font named `DFKai`, Word and LibreOffice fall back to the default system font. When font metrics diverge, paragraph line wraps shift and tables distort.

LocalPDF's internal `docxfont` module scans installed system font families and remaps unrecognized font declarations to the closest available family:
* `w:eastAsia` uses the **localized font name** (e.g. `標楷體`), while `w:ascii` uses the Latin name (e.g. `DFKai-SB`). Real-world benchmarks confirm this provides significantly better fidelity.
* The matcher handles subset prefixes (e.g. `ABCDEF+`) and truncation.

> **Note**: Line wraps will still exhibit minor variance. When `pdf2docx` reassembles PDF lines into semantic paragraphs, Word reflows text according to its own column widths and hyphenation rules. This is an inherent property of document format translation.

### PDF Encryption and Decryption

Powered by `qpdf` (Apache-2.0, ~8 MB bundled), utilizing AES-256 encryption.

* **Passwords are never passed via command-line arguments.** Command-line arguments are visible in the operating system's process table (Task Manager / `ps aux`). LocalPDF communicates with `qpdf` using `@-`: arguments and passwords are fed one per line via standard input (`stdin`), which is immediately closed upon submission. Passwords never appear in process listings and never touch the disk.
* Decryption **requires a valid password**. LocalPDF does not perform password recovery or brute-force cracking.

### Standalone Python Extension (`pyext`)

High-quality PDF to Word conversion currently requires the Python ecosystem (`pdf2docx`). While LibreOffice Writer produces floating absolute-positioned text boxes for every visual line (breaking paragraphs), `pdf2docx` extracts genuine semantic paragraphs and achieves 100% table boundary detection on digital PDFs.

To avoid bloating the base application installer from 25 MB to over 240 MB, Python is packaged as an **optional standalone extension**:

```bash
python scripts/build-pyext.py
```

This script downloads an embeddable Python runtime, installs pinned dependencies, and trims unnecessary modules:
- Replaces standard OpenCV with `opencv-python-headless` (-70 MB)
- Strips unused video codecs from bundled libraries (-29 MB)
- Purges post-install `pip` and `setuptools` (-8 MB)

The extension is downloaded or built **once**. It does not perform runtime network requests; our zero-network privacy guarantee remains intact.

---

## Multi-Tool Pipeline Sequencing

Tools can be combined in a single batch pipeline. When multiple operations are selected, LocalPDF executes them in a **deterministic, canonical order** (`Operation::order`). Intermediate artifacts remain in managed volatile temporary storage and are wiped after each stage:

`Source Format Conversion` → `Repair` → `Page Restructuring (Merge / Select / Rotate / Crop / Split)` → `Overlays (Watermark / Page Numbers / Stamp)` → `Encryption` → `Compression`

Compression is executed last to prevent subsequent modification passes from invalidating image downsampling. Conflicting combinations (e.g., Extract vs. Remove, or mixing interactive Annotation with batch pipelines) are validated and disbarred in the UI before execution.

---

## Dual-Engine Architecture

| Engine | Purpose | Rationale |
| :--- | :--- | :--- |
| **Ghostscript** | Compression, PDF to Image, Repair | Over 80% of PDF file size typically originates from embedded raster imagery. Effective compression requires bicubic downsampling and DCT re-encoding. Structural repair relies on Ghostscript's fault-tolerant PostScript/PDF parser. |
| **lopdf** (Pure Rust) | Merge, Extract, Rotate, Crop, Split, Images to PDF | Structural manipulation only requires editing PDF object graphs. Passing through Ghostscript would rewrite content streams, stripping bookmarks, hyperlinks, and interactive form fields (`AcroForm`). |

* **Images to PDF** uses `lopdf`: JPEG DCT streams are inserted directly as `/DCTDecode` XObjects without re-encoding, preserving 100% original image quality. PNG images are Flate-compressed with an `/SMask` transparency mask to preserve alpha channels.
* **MuPDF (WASM)** runs in the frontend WebView for **Markdown Extraction** (structural font size detection) and the **Annotation & Redaction Editor** (sub-50ms canvas rendering).
* **LibreOffice** is detected on the host system (`soffice.com` on Windows, `soffice` on macOS/Linux) for headless Office to PDF conversion.

---

## Privacy & Security by Design

* **Strict Zero-Network Policy**: Zero HTTP/WebSocket client dependencies. Content-Security-Policy (CSP) is enforced in `tauri.conf.json` to disallow all outbound network calls.
* **Controlled Ephemeral Storage**: Intermediate files are written exclusively to a managed volatile directory:
  - Windows: `%LOCALAPPDATA%\com.localpdf.app\eph\`
  - macOS: `~/Library/Application Support/com.localpdf.app/eph/`
  - Linux: `~/.local/share/com.localpdf.app/eph/`
* **Three-Tier Wipe Guarantee**:
  1. `Ephemeral` RAII `Drop` implementation: intermediate files are zero-filled and unlinked upon error, user cancellation, or unexpected process termination.
  2. Same-device atomic moves (`rename`) avoid duplicating data on disk; cross-device saves immediately wipe the source file.
  3. Startup Sweep: scans and recursively wipes leftover artifacts from system crashes before initializing the UI.
* **Subprocess Sandboxing**: Ghostscript runs with mandatory `-dSAFER` to sandbox PostScript filesystem operations.
* **True Redaction**: The editor's redaction tool utilizes MuPDF's `applyRedactions()` to permanently excise text, vector curves, and raster pixels from the content stream. Saved redacted files are written via full stream rewrites—never incremental updates.

> **Hardware Notice**: On modern SSDs and flash storage, flash translation layers (wear-leveling and TRIM) mean physical sector overwrite cannot be guaranteed at the hardware controller level. True defense-in-depth requires pairing LocalPDF with full-disk encryption (BitLocker, FileVault, or LUKS).

---

## Building from Source

### Prerequisites

- **Node.js** 20+ and `npm`
- **Rust** stable toolchain (`cargo`, `rustup`)
- **Python** 3.11+ (for testing corpus generation and extension packaging)
- **Linux Packages** (Ubuntu / Debian):
  ```bash
  sudo apt-get update
  sudo apt-get install -y \
    libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf \
    ghostscript qpdf fonts-noto-cjk fonts-wqy-zenhei
  ```

### Development Setup

```bash
# 1. Install frontend dependencies (automatically syncs MuPDF WASM runtime)
npm install

# 2. Fetch external engines with verified SHA-256 checksums
npm run fetch:vendor

# 3. Generate benchmark test corpus
pip install reportlab pillow
python bench/make_corpus.py

# 4. Launch development application
npm run tauri dev
```

### Production Build

```bash
npm run tauri build
```

---

## Compression Benchmarks

Compression parameters in `src-tauri/src/gs.rs` are empirically tuned using `bench/compress_bench.py` against representative test corpora:

| Preset | 300dpi Scan (32MB) | Catalog Images (8.8MB) | Text-only (100 Pages) |
| :--- | :--- | :--- | :--- |
| **Extreme** | 5.8% (SSIM 0.908) | 4.1% (SSIM 0.988) | 66% |
| **Recommended** | 11.3% (SSIM 0.944) | 7.0% (SSIM 0.989) | 66% |
| **Low** | 34.5% (SSIM 0.986) | 12.9% (SSIM 0.990) | 66% |

Parameters are balanced using both structural similarity (SSIM) and optical character legibility checks (`bench/legibility_check.py`) to prevent CJK stroke merging.

---

## License

This project is licensed under the **GNU Affero General Public License v3.0 or later** ([AGPL-3.0-or-later](LICENSE)). Bundled binaries and third-party dependencies are documented in [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md).
