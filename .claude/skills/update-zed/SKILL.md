---
name: update-zed
description: Move the Zed with PDF Viewer fork onto a newer stable Zed release - rebase the five fork commits, fix conflicts and API breaks, verify in the real app (including remote projects), keep the commit layout, and publish a release. Use when a new Zed version is out or the user asks to update, sync or rebase onto upstream Zed.
---

# Move the fork to a new Zed release

The authoritative reference is `crates/pdf_viewer/MAINTAINING.md`, sections "Moving to a new Zed release", "Repository layout", "Tests" and "Releasing".
`crates/pdf_viewer/ARCHITECTURE.md` lists every upstream file the fork edits and why.
This skill is the order to do things in and what to check at each step.

## 1. Start clean

- `git switch pdf-viewer && git pull --ff-only` (if the pull fails because the branch was force-pushed, `git fetch && git reset --hard origin/pdf-viewer` after checking you have nothing unpushed).
- `git status` must be clean.
- `cargo` must be rustup's proxy, not a toolchain's `bin/cargo` (see `.claude/CLAUDE.md`); the scripts refuse to run otherwise.
- `script/pdf-viewer/upstream-version` prints the current base, for example `v1.24.2`.
- `git log --oneline "$(script/pdf-viewer/upstream-version)"..pdf-viewer` should show the five fork commits listed in MAINTAINING.md.
  If there are more, fold the extras into the right commit first (see step 5), so the rebase replays the documented layout.
- Pick the target: the newest stable tag (`vX.Y.Z`, never `-pre`) from `git ls-remote --tags --refs https://github.com/zed-industries/zed 'v*'`, unless the user named one.

## 2. Rebase

```bash
script/pdf-viewer/sync-upstream vX.Y.Z
```

It saves the old branch as `pdf-viewer-before-vX.Y.Z`, rebases, and resolves `Cargo.lock` itself.
Exit code 2 means other conflicts: redo the rebase as MAINTAINING.md shows and resolve them.
Each fork commit owns one area, so a conflict tells you where upstream moved:

- `crates/zed/src/main.rs`, `zed.rs`, `visual_test_runner.rs`: put the PDF viewer lines next to the image viewer's again.
- `assets/keymaps/*.json`: keep the `PdfViewer` block right before the `ImageViewer` block.
- `crates/remote/src/transport*.rs`: keep `remote_server_release` and route the server channel and version through it at each call site.
- `Cargo.lock`: `git checkout --ours Cargo.lock && cargo metadata >/dev/null && git add Cargo.lock`.

## 3. Make it build and pass

- `cargo check -p zed --features gpui_platform/runtime_shaders`, then `script/pdf-viewer/check`.
- Compile errors in `crates/pdf_viewer` after a clean rebase mean a GPUI, workspace or project API changed.
  `crates/image_viewer` implements the same traits (`ProjectItem`, `Item`, `SerializableItem`, toolbar) and shows the new API; diff it between the old and new tags.
- `check-remote-server-parity` failing means a fork commit now changes a crate the remote server is built from; move that change out of those crates instead of accepting the failure.
- Fix warnings and flaky tests you meet, even if upstream caused them.

## 4. Verify in the real app

Tests are not enough; run the app the way a user does.

- Local: `ZED_RELEASE_CHANNEL=dev ./target/debug/zed --user-data-dir /tmp/zed-pdf-dev <folder with PDFs>`, open a PDF, scroll, zoom, use the toolbar, split the pane, and rewrite the file on disk to see it reload.
- Remote: follow "Testing remote projects end to end" in MAINTAINING.md (an OrbStack machine over SSH, the official server, open, reload, restart and restore).
- Look at the screenshots in `target/pdf-viewer-visual` that `script/pdf-viewer/check` wrote, and at the real app, with a picky eye: anything that looks off is worth fixing.
- Screenshots of the desktop need the terminal or app to have Screen Recording permission; ask the user if `screencapture` returns a black image.

## 5. Keep the commit layout

- Fold every fix into the commit whose files it changes: `git commit --fixup <commit>`, then `GIT_SEQUENCE_EDITOR=: git rebase -i --autosquash "$(script/pdf-viewer/upstream-version)"`.
- `git log --oneline "$(script/pdf-viewer/upstream-version)"..` must again show exactly the five commits from MAINTAINING.md, unless the update needed a new kind of upstream edit, which gets its own commit plus a row in ARCHITECTURE.md's table and a line in MAINTAINING.md's commit list.
- Update docs that mention the Zed version or anything that changed.

## 6. Publish

Ask the user before pushing, unless they already asked for the update to be published.

```bash
git push --force-with-lease origin pdf-viewer
git push origin vX.Y.Z
script/pdf-viewer/release            # tags vX.Y.Z-pdf.1, builds both DMGs, publishes the release
```

Then disable any upstream workflow the new release brought in (the command is in MAINTAINING.md), and tell the user the release URL and what you verified.
