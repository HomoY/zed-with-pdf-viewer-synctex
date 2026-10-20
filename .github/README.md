# Zed with PDF Viewer (SyncTeX)

[Zed](https://zed.dev), the fast collaborative code editor, with a built-in PDF viewer and SyncTeX for LaTeX: jump from a line of LaTeX to the place it appears in the PDF, and from the PDF back to the line.

This is [Zed with PDF Viewer](https://github.com/TendonT52/zed-with-pdf-viewer) by TendonT52 plus SyncTeX support.
The PDF viewer itself, and everything else described below, comes from that fork; this one adds the jumps between LaTeX sources and their PDF.

> This is an unofficial community fork.
> It is not affiliated with or endorsed by Zed Industries, or by the maintainer of Zed with PDF Viewer.
> For anything not related to PDFs, use [zed-industries/zed](https://github.com/zed-industries/zed).

## SyncTeX

- **PDF to LaTeX:** Cmd-click (Ctrl-click on Linux and Windows) a line in a PDF tab, and Zed opens the LaTeX file at that line, in a pane other than the PDF's.
- **LaTeX to PDF:** press Cmd + Alt + J (Ctrl + Alt + J on Linux and Windows) in a LaTeX file, or run `pdf viewer: show in pdf` from the command palette.
  The PDF tab scrolls to the line under the cursor and highlights it for two seconds, without taking keyboard focus; if the PDF isn't open yet, it opens beside the editor.
- Documents split into several files with `\input` or `\include` work: the PDF is looked up among the PDFs open in Zed, the PDF named after the file, and the PDFs with SyncTeX data in the file's folder and its parent folders up to the project root.
- Remote projects over SSH, WSL and containers work too: `synctex` then runs on the remote machine, through your login shell there, so a TeX distribution installed in your home directory is found.

You need:

- the `synctex` command, which comes with TeX Live, MacTeX and MiKTeX, on the machine that has the files;
- a PDF built with SyncTeX data, such as `latexmk -pdf -synctex=1`.
  The default build command of the [LaTeX extension](https://zed.dev/extensions/latex) does this.

Cmd + Alt + J is the forward-search key of LaTeX Workshop in VS Code.
In LaTeX files it replaces Zed's own use of the key, toggling the workspace sidebar, which keeps working everywhere else.
To use another key, bind `pdf_viewer::ShowInPdf` in your keymap (`zed: open keymap file`):

```json
[
  {
    "context": "Editor && extension == tex",
    "bindings": {
      "ctrl-cmd-j": "pdf_viewer::ShowInPdf"
    }
  }
]
```

## Features of the PDF viewer

- Opens `.pdf` files from the project panel, the file finder, or the command line like any other file.
- Works in remote projects over SSH, WSL and containers: PDFs on the remote machine open, live reload and come back after a restart like local ones.
- Continuous scrolling with sharp text at any zoom level, including Retina displays.
- Zoom with Cmd + scroll, trackpad pinch, or the toolbar, from 10% to 1600%, anchored at the cursor.
- Fit width, fit page, and actual size modes.
- Page navigation: previous and next page buttons, arrow keys, Page Up and Page Down, Space, and jumping to a typed page number (Ctrl+G).
- Live reload: when the PDF changes on disk, for example after LaTeX or Typst recompiles it, the view updates in place and keeps your position.
- Drag to pan, split panes, and session restore of the page and zoom you were at.
- Pages always show the document's original colors.

## Install on macOS

1. Download the DMG for your Mac from the [latest release](https://github.com/HomoY/zed-with-pdf-viewer-synctex/releases/latest): `aarch64` for Apple Silicon, `x86_64` for Intel.
2. Open the DMG and drag **Zed with PDF Viewer (SyncTeX)** into Applications.
3. The app is not notarized by Apple, so on first launch macOS says it "could not verify" the app.
   Click **Done** (not Move to Trash), then either:
   - open System Settings, Privacy & Security, scroll down and click **Open Anyway**, or
   - run this once in Terminal:

     ```bash
     xattr -dr com.apple.quarantine "/Applications/Zed with PDF Viewer (SyncTeX).app"
     ```

     If it prints "Operation not permitted", macOS is asking for permission to modify apps: allow your terminal under System Settings, Privacy & Security, App Management (or accept the prompt), then run it again.

The app installs next to the official Zed and Zed with PDF Viewer, and doesn't replace them.
Remote projects over SSH, WSL and containers work as in official Zed: the app installs the official Zed remote server of the same release on the remote machine.
All of them share your settings, keymaps, themes and extensions in `~/.config/zed`.
This app and Zed with PDF Viewer also share their open projects and session, so don't run both at once.
It does not auto-update itself; new versions appear on the releases page.

## Keyboard shortcuts

| Action | macOS | Linux and Windows |
| --- | --- | --- |
| Jump from the PDF to the LaTeX source | Cmd + click | Ctrl + click |
| Jump from a LaTeX file to the PDF | Cmd + Alt + J | Ctrl + Alt + J |
| Zoom in and out | Cmd + = and Cmd + - | Ctrl + = and Ctrl + - |
| Fit width | Cmd + 0 | Ctrl + 0 |
| Actual size | Cmd + 1 | Ctrl + 1 |
| Fit page | Cmd + Shift + 0 | Ctrl + Shift + 0 |
| Previous and next page | Left and Right | Left and Right |
| First and last page | Home and End, Cmd + Up and Cmd + Down | Home and End, Ctrl + Home and Ctrl + End |
| Go to page | Ctrl + G | Ctrl + G |
| Scroll | Up, Down, Page Up, Page Down, Space, Shift + Space | Same |

All actions are also in the command palette under `pdf viewer:`.

## Build from source

You need Rust (the version in `rust-toolchain.toml` is installed automatically by rustup), CMake, and on macOS the Xcode command line tools.

```bash
git clone --branch pdf-viewer https://github.com/HomoY/zed-with-pdf-viewer-synctex.git
cd zed-with-pdf-viewer-synctex
script/pdf-viewer/fetch-pdfium
cargo run --features gpui_platform/runtime_shaders
```

`fetch-pdfium` downloads a checksum-verified Pdfium library into `~/.local/share/kkpdf-zed/lib`.
`runtime_shaders` is only needed when full Xcode with its Metal compiler is not installed.

To build the macOS app and DMG yourself, run `script/pdf-viewer/bundle-mac`.
The DMG is written to `target/pdf-viewer/`.

The viewer code is cross-platform, but only macOS builds are published and tested so far.

## How it relates to Zed with PDF Viewer

The `pdf-viewer` branch is [TendonT52/zed-with-pdf-viewer](https://github.com/TendonT52/zed-with-pdf-viewer) plus the SyncTeX commits on top.
Releases are tagged after the release they are based on, for example `v1.24.2-pdf.4-synctex.1` for SyncTeX on top of Zed with PDF Viewer 1.24.2-pdf.4.
Moving to newer releases is done by hand, so a build can appear some time after the release it is based on.

The SyncTeX code lives in `crates/pdf_viewer/src/synctex.rs`, with small additions to `crates/pdf_viewer/src/pdf_view.rs`; see the SyncTeX section of [crates/pdf_viewer/ARCHITECTURE.md](../crates/pdf_viewer/ARCHITECTURE.md).

## Credits and license

- [Zed](https://github.com/zed-industries/zed) by Zed Industries, licensed under GPL-3.0-or-later (see `LICENSE-GPL`), which this fork is distributed under as well.
- [Zed with PDF Viewer](https://github.com/TendonT52/zed-with-pdf-viewer) by TendonT52, the PDF viewer this fork builds on, licensed under GPL-3.0-or-later.
- The PDF engine is a modified copy of [kkpdf-zed](https://github.com/kk376/kkpdf-zed) by Kushagra Kumar, licensed under Apache-2.0 or GPL-3.0-or-later.
  See [crates/kkpdf_zed/NOTICE.md](../crates/kkpdf_zed/NOTICE.md) for what changed.
- [Pdfium](https://pdfium.googlesource.com/pdfium/) by Google, licensed under BSD-3-Clause and Apache-2.0, via [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries) and [pdfium-render](https://github.com/ajrcarey/pdfium-render).
- [SyncTeX](https://github.com/jlaurens/synctex) by Jérôme Laurens, whose `synctex` command does the source and PDF lookups.
