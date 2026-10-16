# Zed with PDF Viewer (fork)

This repository is a fork of Zed that adds a built-in PDF viewer.
The root `CLAUDE.md` is upstream Zed's `.rules`, written for contributors to zed-industries/zed; this file covers what is different in the fork.

- Default branch: `pdf-viewer`, which is an upstream Zed release tag plus five fork commits grouped by purpose.
  `main` is an untouched copy of upstream; never commit to it.
- Read `crates/pdf_viewer/ARCHITECTURE.md` (how the viewer works, every upstream file the fork edits) and `crates/pdf_viewer/MAINTAINING.md` (setup, tests, releases, moving to new Zed releases) before changing anything.
- To move the fork to a new Zed release, use the `/update-zed` skill.
- There is no CI and no GitHub Actions workflow.
  `script/pdf-viewer/check` runs every test and check and must pass before a push; `script/pdf-viewer/release` builds the DMGs on this Mac and publishes the GitHub release.
- Keep PDF changes inside `crates/pdf_viewer` and `crates/kkpdf_zed`.
  Fold fixes into the fork commit whose files they change, keep each kind of upstream edit in its own commit, and force-push `pdf-viewer`.
- Upstream's "HARD RULE" about a review banner in `README.md` does not apply to the fork, and the fork never edits `.rules`.
- Commit messages have no AI co-author trailers.
- Run `cargo` through rustup's proxies, never from a toolchain's `bin` directory: the proxy sets the library path that `rust-objcopy` needs, and without it every release build warns that stripping debug info failed.
  If `cargo` is not on `PATH`, Homebrew's rustup is keg-only, so use `PATH="/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"`.
