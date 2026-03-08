# LocalPDF

[![CI](https://github.com/Che55577/PDFeditor/actions/workflows/ci.yml/badge.svg)](https://github.com/Che55577/PDFeditor/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/Che55577/PDFeditor?include_prereleases&label=Release)](https://github.com/Che55577/PDFeditor/releases)
[![License: AGPL-3.0](https://img.shields.io/badge/License-AGPL--3.0-blue.svg)](LICENSE)
![Platform: Windows | macOS | Linux](https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey)

**English** | [繁體中文](README.md)

**LocalPDF** is a privacy-first, **100% offline cross-platform desktop PDF toolbox**. Every compression, merge, conversion, encryption, and redaction operation happens strictly on your local machine—your documents never leave your device, and all intermediate temporary files are zero-filled and unlinked immediately upon saving or closing the window.

---

## Key Highlights

* **100% Offline & Zero Network Connectivity**: Zero HTTP/WebSocket client dependencies, reinforced by a strict Content-Security-Policy (CSP) blocking all outbound network requests—built for confidential contracts, financial statements, medical records, and legal documents.
* **18-in-1 PDF Toolbox**: Covers daily document workflows including compression, page restructuring, format conversion, AES-256 encryption, watermarks/stamps, and interactive annotations.
* **Smart Multi-Step Pipeline Chaining**: Select multiple tools at once (e.g., `Rotate → Page Numbers → Watermark → Compress`) and LocalPDF automatically executes them in their optimal canonical sequence, wiping intermediate files after each stage.
* **True Content-Stream Redaction**: Permanently excises text, vector paths, and raster pixels from the underlying PDF content stream and forces a full file rewrite—eliminating the risk of "black rectangles" where hidden text can still be copied out.
* **Lightweight Core + Modular Extensions**: The base installer is only ~25 MB (bundling Ghostscript, qpdf, `lopdf`, and MuPDF WASM). Users who need **PDF to Word** can import the standalone offline Python extension (`pyext`) with one click directly inside the app.

---

## Feature Matrix (18 Tools)

| Category | Tool | Description | Underlying Engine |
| :--- | :--- | :--- | :--- |
| **Compress & Repair** | **Compress PDF** | Three empirically tuned presets (Extreme, Recommended, Low) balancing size reduction and text legibility | Ghostscript (`pdfwrite`) |
| | **Repair PDF** | Reconstructs broken cross-reference tables (`xref`) and damaged object streams | Ghostscript |
| **Organize Pages** | **Merge PDF** | Combines multiple PDFs in drag-and-drop order while preserving bookmarks (`Outlines`) and form fields (`AcroForm`) | `lopdf` (Pure Rust) |
| | **Split PDF** | Splits large PDFs by fixed page chunks or custom page ranges into separate files | `lopdf` |
| | **Extract Pages** | Extracts specific pages or ranges (e.g., `1, 3, 5-10`) into a new PDF | `lopdf` |
| | **Remove Pages** | Deletes selected pages and prunes orphaned image objects (`prune_objects`) to genuinely shrink file size | `lopdf` |
| | **Rotate Pages** | Rotates all or selected pages clockwise by 90°, 180°, or 270° | `lopdf` |
| | **Crop Margins** | Trims top/bottom/left/right margins (in `pt`), supporting inherited `MediaBox` and `CropBox` attributes | `lopdf` |
| **Convert Formats** | **PDF to Images** | Renders each page into high-resolution JPG or lossless PNG images (20–600 DPI) | Ghostscript |
| | **Images to PDF** | Packages JPGs (lossless direct `/DCTDecode` embedding, zero re-encoding) and PNGs (with alpha transparency) into PDF | `lopdf` + `image` |
| | **PDF to Markdown** | Extracts structured text and automatically infers `# / ## / ###` heading levels from font sizes | MuPDF WASM / Ghostscript |
| | **PDF to Word** | Reconstructs true paragraphs and tables into `.docx`, preserving barcodes/QR codes and remapping CJK fonts | `pyext` (`pdf2docx` + `docxfont`) |
| | **Office to PDF** | Batch-converts Word (`.doc`/`.docx`), Excel (`.xls`/`.xlsx`), and PowerPoint (`.ppt`/`.pptx`) files to PDF | LibreOffice Headless |
| **Security & Redaction** | **Encrypt PDF** | Protects PDFs with AES-256 encryption; passwords are piped via `stdin` and never exposed in process tables | `qpdf` (`@-`) |
| | **Decrypt PDF** | Removes password protection when given the valid password for archiving or downstream processing | `qpdf` (`@-`) |
| | **True Redaction** | Draw redaction boxes on the canvas to permanently strip underlying text and images from the PDF stream | MuPDF WASM |
| **Annotate & Overlay** | **Watermark** | Overlays angled, customizable text watermarks with automatic TrueType font subsetting (`subsetter`) | `lopdf` + `subsetter` |
| | **Page Numbers** | Adds customizable `{n} / {total}` page numbers at any of 9 grid positions with custom starting numbers | `lopdf` + `subsetter` |
| | **Stamp / Signature** | Overlays transparent PNG signatures or company stamps (`/SMask` alpha support) onto target pages | `lopdf` + `/SMask` |
| | **Interactive Annotate** | Visual page viewer supporting FreeText annotations, freehand ink drawing, and true redaction | MuPDF WASM |

---

## Download & Installation

Download the latest installer for your operating system from [**GitHub Releases**](https://github.com/Che55577/PDFeditor/releases):

| Operating System | Package Format | Installation Notes |
| :--- | :--- | :--- |
| **Windows (x64)** | `LocalPDF_*_x64-setup.exe` | Run the NSIS installer (bundles Ghostscript and qpdf out-of-the-box) |
| **macOS (Apple Silicon / Intel)** | `LocalPDF_*.dmg` | Drag `LocalPDF.app` to `Applications` (see Gatekeeper note below for first launch) |
| **Linux (Ubuntu / Debian)** | `LocalPDF_*_amd64.deb` / `.AppImage` | `sudo apt install ./LocalPDF_*_amd64.deb` (uses system `ghostscript` and `qpdf`) |

### Enabling Optional Extensions

1. **Enable "PDF to Word" (Standalone Python Extension `pyext`)**:
   - Download the extension archive for your platform from [GitHub Releases](https://github.com/Che55577/PDFeditor/releases) (e.g., `localpdf-pyext-Windows-X64.zip` or `localpdf-pyext-macOS-ARM64.zip`).
   - Open LocalPDF, click the **"Import Extension (.zip)"** button in the footer status bar or inside the "PDF to Word" panel, and select the downloaded `.zip` file. LocalPDF will verify and install it into your user data directory immediately—no administrator privileges or app restart required.
2. **Enable "Office to PDF" (LibreOffice)**:
   - LocalPDF automatically detects system installations of LibreOffice. If not installed:
     ```bash
     # Windows (PowerShell)
     winget install --id TheDocumentFoundation.LibreOffice -e

     # macOS (Homebrew)
     brew install --cask libreoffice

     # Ubuntu / Debian
     sudo apt-get install -y libreoffice
     ```

### First-Launch Guide for Unsigned Builds & Provenance Verification

Because LocalPDF is an independent open-source project distributed without paid commercial code-signing certificates, your OS may display an unsigned binary prompt on first launch:

* **macOS ("LocalPDF is damaged and can't be opened" or "unidentified developer")**:
  After dragging `LocalPDF.app` to `/Applications`, clear the browser quarantine attribute in Terminal:
  ```bash
  xattr -cr /Applications/LocalPDF.app
  ```
* **Windows ("Windows protected your PC" SmartScreen prompt)**:
  Click **"More info"** → **"Run anyway"**.
* **Verify Cryptographic Build Provenance & Checksums**:
  Every release includes `SHA256SUMS.txt` and a Sigstore cryptographic build provenance attestation generated by GitHub Actions. You can verify that your installer was built directly from this repository's public source code using the GitHub CLI:
  ```bash
  gh attestation verify <downloaded-installer-file> --repo Che55577/PDFeditor
  ```

---

## Privacy & Security Architecture

LocalPDF is engineered under the assumption that **every document processed is highly confidential and every untrusted input file may be malformed**:

1. **Three-Tier Ephemeral File Wiping**:
   - **Isolated Volatile Directory**: Intermediate files are written exclusively to a managed directory with random extensionless filenames (preventing OS thumbnailers or desktop search indexers from inspecting them).
   - **Zero-Fill on Drop**: Whether an operation succeeds, fails, is cancelled by the user, or panics, the `Ephemeral` RAII guard and `wipe_dir_all` overwrite file contents with `0x00` bytes and flush (`sync_all`) before unlinking.
   - **Startup Sweep**: Every launch automatically sweeps and wipes any leftover temporary files from prior power loss or forced termination.
2. **Process-Table Safe Passwords**:
   - Encryption and decryption invoke `qpdf @-`, feeding arguments and passwords over standard input (`stdin`) and immediately closing the pipe—ensuring passwords never appear in OS process listings (`ps` / Task Manager).
3. **Subprocess Sandboxing & Resource Guards**:
   - Ghostscript runs with mandatory `-dSAFER` to block PostScript filesystem access; `.docx` font remapping enforces strict 64 MB per-entry and 256 MB total extraction limits against Zip Bombs; all child processes run under timeout and background pipe-drain threads with instant user cancellation.

> **Hardware Notice**: On modern SSDs and flash media, controller-level wear-leveling and TRIM mean software zero-filling cannot guarantee immediate physical flash cell erasure. For protection against physical disk forensics, always pair LocalPDF with full-disk encryption (BitLocker, FileVault, or LUKS).

---

## Frequently Asked Questions (FAQ)

<details>
<summary><b>Q1: Can I directly edit existing body paragraphs inside a PDF?</b></summary>

The PDF specification stores positioned glyph runs with absolute coordinates rather than reflowable paragraphs. Therefore:
- To **add text callouts, fill forms, sign, or redact sensitive areas**: use the built-in **"Annotate"** tool.
- To **edit body text and reflow paragraphs**: convert the file using **"PDF to Word"** (`.docx`) → edit in Microsoft Word or LibreOffice Writer → convert back using **"Office to PDF"**. LocalPDF provides both ends of this offline workflow.
</details>

<details>
<summary><b>Q2: Why is "PDF to Word" packaged as a separate download?</b></summary>

Accurate paragraph and table reconstruction requires Python's `pdf2docx`, `PyMuPDF`, and `OpenCV` runtime (~216 MB after trimming). Keeping it as an optional one-click extension allows the main LocalPDF installer to remain lightweight (~25 MB) for users who only need compression, merging, encryption, and annotations.
</details>

<details>
<summary><b>Q3: What is the difference between drawing a black box and using "Redact"?</b></summary>

**Never cover sensitive text with a regular drawing brush or shape!** A visual overlay leaves the underlying text bytes intact in the PDF stream—anyone can still copy-paste or search (`Ctrl+F`) the hidden text.
Always use the **"Redact"** mode in the Annotation Editor: it invokes `applyRedactions()` to permanently delete text, images, and vector paths inside the marked region and performs a full PDF stream rewrite.
</details>

---

## Building from Source & Contributing

For an in-depth look at internal engine design, empirical compression benchmarks, and font subsetting internals, see [**`docs/ARCHITECTURE.md`**](docs/ARCHITECTURE.md).

### Prerequisites
- **Node.js** 20+ (`npm`)
- **Rust** stable toolchain (`cargo`)
- **Python** 3.11+ (for test corpus generation and extension packaging)
- **Linux System Libraries** (Ubuntu / Debian):
  ```bash
  sudo apt-get update && sudo apt-get install -y \
    libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf \
    ghostscript qpdf fonts-noto-cjk fonts-wqy-zenhei
  ```

### Quick Start & Automated Tests

```bash
# 1. Install frontend dependencies (syncs MuPDF WASM runtime to public/mupdf/)
npm ci

# 2. Fetch external engines with SHA-256 verification
npm run fetch:vendor

# 3. Run frontend UI unit/integration tests and production build check
npm run test:ui
npm run build

# 4. Generate test corpus and run Rust integration tests
pip install reportlab pillow
python bench/make_corpus.py
cargo test --manifest-path src-tauri/Cargo.toml

# 5. Start desktop development mode
npm run tauri dev
```

Contributions are welcome! Please read [**CONTRIBUTING.md**](CONTRIBUTING.md) and [**SECURITY.md**](SECURITY.md) before submitting a Pull Request.

---

## License

Licensed under the [**GNU Affero General Public License v3.0 or later (AGPL-3.0-or-later)**](LICENSE). Third-party open-source component licenses and notices are documented in [**THIRD_PARTY_NOTICES.md**](THIRD_PARTY_NOTICES.md).
