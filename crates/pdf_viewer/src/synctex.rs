//! SyncTeX: jumping between LaTeX sources and the PDF they were compiled into.
//!
//! Both directions run the `synctex` command-line tool that comes with TeX Live and
//! MiKTeX. It reads the `.synctex.gz` file that the compiler writes next to the PDF
//! when run with `-synctex=1` (latexmk and texlab builds usually do). In remote
//! projects it runs on the remote machine, over the project's connection.
//! Positions in a PDF are in points from the top-left corner of a page, the same
//! convention as [`PageAnchor`](crate::layout::PageAnchor).

use std::{
    cmp::Reverse,
    collections::HashSet,
    ffi::OsString,
    path::{Path, PathBuf},
    time::SystemTime,
};

use anyhow::{Context as _, Result, bail};
use editor::Editor;
use gpui::{App, AppContext as _, Context, Task, TaskExt as _, Window, actions};
use language::Point;
use project::Project;
use remote::{Interactive, RemoteClient};
use workspace::{OpenOptions, OpenVisible, Toast, Workspace, notifications::NotificationId};

use crate::pdf_view::PdfView;

actions!(
    pdf_viewer,
    [
        /// Show the text at the cursor in the PDF compiled from this LaTeX file, using SyncTeX.
        ShowInPdf,
    ]
);

/// Where TeX distributions install `synctex`, for when it isn't on `PATH`.
const FALLBACK_DIRS: &[&str] = &[
    "/Library/TeX/texbin",
    "/usr/texbin",
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
];

/// Width of the box shown when SyncTeX reports a point but no enclosing box.
const POINT_BOX_SIZE: f32 = 10.;
/// Boxes taller than this are paragraphs or columns rather than lines of text.
const MAX_LINE_BOX_HEIGHT: f32 = 24.;
/// Height above the baseline of the box shown around a reference point.
const POINT_LINE_HEIGHT: f32 = 10.;

/// A box in the PDF that a source line was typeset into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PdfBox {
    pub page_index: usize,
    /// Left edge, in points from the page's left edge.
    pub x: f32,
    /// Top edge, in points from the page's top edge.
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// A line of a source file, as reported by `synctex edit`.
#[derive(Debug, Clone, PartialEq)]
pub struct SourceLocation {
    pub path: PathBuf,
    /// Zero-based line number.
    pub row: u32,
}

/// One record of `synctex view` output.
#[derive(Default)]
struct ViewRecord {
    page: Option<usize>,
    x: Option<f32>,
    y: Option<f32>,
    h: Option<f32>,
    v: Option<f32>,
    width: Option<f32>,
    height: Option<f32>,
}

impl ViewRecord {
    fn into_box(self) -> Option<PdfBox> {
        let page_index = self.page?.checked_sub(1)?;
        match (self.h, self.v, self.width, self.height) {
            // `v` is the baseline and the height is counted upwards from it.
            (Some(h), Some(v), Some(width), Some(height))
                if width > 0. && height > 0. && height <= MAX_LINE_BOX_HEIGHT =>
            {
                Some(PdfBox {
                    page_index,
                    x: h,
                    y: v - height,
                    width,
                    height,
                })
            }
            // Some SyncTeX versions report the enclosing column or paragraph, so the
            // line is taken from the reference point, whose `y` is its baseline.
            _ => {
                let (x, y) = (self.x?, self.y?);
                let (left, width) = match (self.h, self.width) {
                    (Some(h), Some(width)) if width > 0. => (h, width),
                    _ => (x - POINT_BOX_SIZE / 2., POINT_BOX_SIZE),
                };
                Some(PdfBox {
                    page_index,
                    x: left,
                    y: y - POINT_LINE_HEIGHT,
                    width,
                    height: POINT_LINE_HEIGHT,
                })
            }
        }
    }
}

/// Parses the records printed by `synctex view`, keeping those on the first record's page.
pub fn parse_view_output(output: &str) -> Vec<PdfBox> {
    let mut records = Vec::new();
    let mut current: Option<ViewRecord> = None;
    for line in output.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key == "Output" {
            records.extend(current.take());
            current = Some(ViewRecord::default());
            continue;
        }
        let Some(record) = current.as_mut() else {
            continue;
        };
        let value = value.trim();
        match key {
            "Page" => record.page = value.parse().ok(),
            "x" => record.x = value.parse().ok(),
            "y" => record.y = value.parse().ok(),
            "h" => record.h = value.parse().ok(),
            "v" => record.v = value.parse().ok(),
            "W" => record.width = value.parse().ok(),
            "H" => record.height = value.parse().ok(),
            _ => {}
        }
    }
    records.extend(current);

    let boxes: Vec<PdfBox> = records
        .into_iter()
        .filter_map(ViewRecord::into_box)
        .collect();
    let Some(first_page) = boxes.first().map(|pdf_box| pdf_box.page_index) else {
        return boxes;
    };
    boxes
        .into_iter()
        .filter(|pdf_box| pdf_box.page_index == first_page)
        .collect()
}

/// Parses the first record printed by `synctex edit`.
pub fn parse_edit_output(output: &str) -> Option<SourceLocation> {
    let mut path = None;
    let mut line = None;
    for output_line in output.lines() {
        if let Some(value) = output_line.strip_prefix("Input:") {
            // SyncTeX reports paths as TeX saw them, such as `/project/./content/a.tex`.
            path.get_or_insert_with(|| Path::new(value.trim()).components().collect::<PathBuf>());
        } else if let Some(value) = output_line.strip_prefix("Line:") {
            if line.is_none() {
                line = value.trim().parse::<u32>().ok();
            }
        }
        if path.is_some() && line.is_some() {
            break;
        }
    }
    Some(SourceLocation {
        path: path?,
        row: line?.saturating_sub(1),
    })
}

fn synctex_executable() -> Result<PathBuf> {
    let name = if cfg!(windows) {
        "synctex.exe"
    } else {
        "synctex"
    };
    let path_dirs = std::env::var_os("PATH")
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    path_dirs
        .into_iter()
        .chain(FALLBACK_DIRS.iter().map(PathBuf::from))
        .map(|dir| dir.join(name))
        .find(|path| path.is_file())
        .context("couldn't find the `synctex` command; it comes with TeX distributions such as MacTeX and TeX Live")
}

/// A `synctex` invocation, run on this computer or, for remote projects, on the
/// remote machine.
pub struct SyncTexCommand {
    /// The program to run; `None` runs `synctex` on this computer.
    program: Option<String>,
    args: Vec<OsString>,
    env: Vec<(String, String)>,
}

impl SyncTexCommand {
    /// Runs `synctex` with `args` here, or on the machine behind `remote_client`.
    /// There it goes through the user's login shell, which is where TeX
    /// distributions installed in a home directory usually add themselves to `PATH`.
    fn new(remote_client: Option<&RemoteClient>, args: Vec<OsString>) -> Result<Self> {
        let Some(remote_client) = remote_client else {
            return Ok(Self {
                program: None,
                args,
                env: Vec::new(),
            });
        };
        let shell = remote_client.shell().unwrap_or_else(|| "sh".to_string());
        // The arguments reach `synctex` through `"$@"` (`$argv` in fish), so the shell
        // doesn't interpret them a second time.
        let mut shell_args = vec!["-l".to_string(), "-c".to_string()];
        if shell.ends_with("fish") {
            shell_args.push("exec synctex $argv".to_string());
        } else {
            shell_args.push("exec synctex \"$@\"".to_string());
            shell_args.push("synctex".to_string());
        }
        shell_args.extend(args.iter().map(|arg| arg.to_string_lossy().into_owned()));
        let command = remote_client.build_command(
            Some(shell),
            &shell_args,
            &Default::default(),
            None,
            None,
            Interactive::No,
        )?;
        Ok(Self {
            program: Some(command.program),
            args: command.args.into_iter().map(OsString::from).collect(),
            env: command.env.into_iter().collect(),
        })
    }

    /// Finds where line `row` (zero-based) of `source` was typeset in `pdf`.
    pub fn view(
        remote_client: Option<&RemoteClient>,
        source: &Path,
        row: u32,
        pdf: &Path,
    ) -> Result<Self> {
        // SyncTeX ignores the column in practice, so it is left out (0).
        let mut input = OsString::from(format!("{}:0:", row + 1));
        input.push(source);
        Self::new(
            remote_client,
            vec!["view".into(), "-i".into(), input, "-o".into(), pdf.into()],
        )
    }

    /// Finds the source line typeset at a point of `pdf`.
    pub fn edit(
        remote_client: Option<&RemoteClient>,
        pdf: &Path,
        page_index: usize,
        x: f32,
        y: f32,
    ) -> Result<Self> {
        let mut target = OsString::from(format!("{}:{x}:{y}:", page_index + 1));
        target.push(pdf);
        Self::new(remote_client, vec!["edit".into(), "-o".into(), target])
    }

    /// Runs the command and returns what it printed.
    pub async fn run(self) -> Result<String> {
        let program = match self.program {
            Some(program) => PathBuf::from(program),
            None => synctex_executable()?,
        };
        let output = util::command::new_command(&program)
            .args(&self.args)
            .envs(self.env)
            .output()
            .await
            .with_context(|| format!("couldn't run {}", program.display()))?;
        if !output.status.success() {
            bail!(
                "synctex failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// The stem of a SyncTeX file name, such as `main` for `main.synctex.gz`.
fn synctex_stem(file_name: &str) -> Option<&str> {
    file_name
        .strip_suffix(".synctex.gz")
        .or_else(|| file_name.strip_suffix(".synctex"))
}

/// How [`candidate_pdfs`] sees the files around a source file.
trait FileListing {
    fn is_file(&self, path: &Path) -> bool;

    /// Stems of the SyncTeX files directly in `dir` with their modification times, or
    /// `None` when `dir` is outside the project and the search should stop.
    fn synctex_files(&self, dir: &Path) -> Option<Vec<(SystemTime, String)>>;
}

/// Files on this computer, searched up to `root`.
struct LocalFiles<'a> {
    root: &'a Path,
}

impl FileListing for LocalFiles<'_> {
    fn is_file(&self, path: &Path) -> bool {
        path.is_file()
    }

    fn synctex_files(&self, dir: &Path) -> Option<Vec<(SystemTime, String)>> {
        if !dir.starts_with(self.root) {
            return None;
        }
        let entries = std::fs::read_dir(dir).into_iter().flatten().flatten();
        Some(
            entries
                .filter_map(|entry| {
                    let name = entry.file_name().into_string().ok()?;
                    let stem = synctex_stem(&name)?.to_string();
                    let modified = entry.metadata().and_then(|metadata| metadata.modified());
                    Some((modified.unwrap_or(SystemTime::UNIX_EPOCH), stem))
                })
                .collect(),
        )
    }
}

/// Files in a project's worktrees, which may be on another machine.
struct ProjectFiles<'a> {
    project: &'a Project,
    cx: &'a App,
}

impl FileListing for ProjectFiles<'_> {
    fn is_file(&self, path: &Path) -> bool {
        self.project
            .find_project_path(path, self.cx)
            .and_then(|project_path| self.project.entry_for_path(&project_path, self.cx))
            .is_some_and(|entry| entry.is_file())
    }

    fn synctex_files(&self, dir: &Path) -> Option<Vec<(SystemTime, String)>> {
        let dir = self.project.find_project_path(dir, self.cx)?;
        let worktree = self
            .project
            .worktree_for_id(dir.worktree_id, self.cx)?
            .read(self.cx);
        Some(
            worktree
                .child_entries(&dir.path)
                .filter_map(|entry| {
                    let stem = synctex_stem(entry.path.file_name()?)?.to_string();
                    let modified = entry
                        .mtime
                        .map_or(SystemTime::UNIX_EPOCH, |mtime| mtime.timestamp_for_user());
                    Some((modified, stem))
                })
                .collect(),
        )
    }
}

/// PDFs with SyncTeX data that `source` may have been compiled into, most likely first:
/// the given open PDFs, the PDF named after `source`, then PDFs next to `source` or in
/// its parent directories within the project, most recently built first.
fn candidate_pdfs(
    source: &Path,
    open_pdfs: Vec<PathBuf>,
    files: &impl FileListing,
) -> Vec<PathBuf> {
    let mut candidates = open_pdfs;
    candidates.push(source.with_extension("pdf"));
    for dir in source.ancestors().skip(1) {
        let Some(mut built) = files.synctex_files(dir) else {
            break;
        };
        built.sort_by_key(|(modified, _)| Reverse(*modified));
        candidates.extend(
            built
                .into_iter()
                .map(|(_, stem)| dir.join(format!("{stem}.pdf"))),
        );
    }

    let has_synctex_data = |pdf: &Path| {
        files.is_file(&pdf.with_extension("synctex.gz"))
            || files.is_file(&pdf.with_extension("synctex"))
    };
    let mut seen = HashSet::new();
    candidates
        .retain(|pdf| seen.insert(pdf.clone()) && files.is_file(pdf) && has_synctex_data(pdf));
    candidates
}

fn show_message(workspace: &mut Workspace, message: String, cx: &mut Context<Workspace>) {
    workspace.show_toast(
        Toast::new(NotificationId::unique::<ShowInPdf>(), message).autohide(),
        cx,
    );
}

/// Handles [`ShowInPdf`]: finds the PDF line of the cursor in the active editor and
/// scrolls a PDF tab to it, opening the PDF beside the editor if it isn't open.
pub(crate) fn show_in_pdf(
    workspace: &mut Workspace,
    _: &ShowInPdf,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let Some(editor) = workspace.active_item_as::<Editor>(cx) else {
        return;
    };
    let Some((source, row)) = editor.update(cx, |editor, cx| {
        let row = editor
            .selections
            .newest::<Point>(&editor.display_snapshot(cx))
            .head()
            .row;
        Some((editor.target_file_abs_path(cx)?, row))
    }) else {
        return;
    };

    // PDFs shown in a pane come first, then the other open ones.
    let mut open_pdfs = Vec::new();
    for active in [true, false] {
        for pane in workspace.panes() {
            let pane = pane.read(cx);
            for view in pane.items_of_type::<PdfView>() {
                let is_active = pane
                    .active_item()
                    .is_some_and(|item| item.item_id() == view.entity_id());
                if is_active == active {
                    open_pdfs.push(view.read(cx).path_on_host(cx));
                }
            }
        }
    }

    // Remote projects are searched through their worktrees, and `synctex` runs on the
    // remote machine; local ones are searched on disk in the background.
    let project = workspace.project().read(cx);
    let commands: Task<Result<Vec<(PathBuf, SyncTexCommand)>>> = match project.remote_client() {
        Some(remote_client) => {
            let remote_client = remote_client.read(cx);
            let files = ProjectFiles { project, cx };
            Task::ready(
                candidate_pdfs(&source, open_pdfs, &files)
                    .into_iter()
                    .map(|pdf| {
                        let command =
                            SyncTexCommand::view(Some(remote_client), &source, row, &pdf)?;
                        Ok((pdf, command))
                    })
                    .collect(),
            )
        }
        None => {
            let root = project
                .find_project_path(&source, cx)
                .and_then(|project_path| project.worktree_for_id(project_path.worktree_id, cx))
                .map(|worktree| worktree.read(cx).abs_path().to_path_buf())
                .or_else(|| source.parent().map(Path::to_path_buf))
                .unwrap_or_default();
            cx.background_spawn(async move {
                candidate_pdfs(&source, open_pdfs, &LocalFiles { root: &root })
                    .into_iter()
                    .map(|pdf| {
                        let command = SyncTexCommand::view(None, &source, row, &pdf)?;
                        Ok((pdf, command))
                    })
                    .collect()
            })
        }
    };

    cx.spawn_in(window, async move |workspace, cx| {
        let commands = commands.await;
        let found = cx
            .background_spawn(async move {
                for (pdf, command) in commands? {
                    let boxes = parse_view_output(&command.run().await?);
                    if !boxes.is_empty() {
                        return Ok(Some((pdf, boxes)));
                    }
                }
                anyhow::Ok(None)
            })
            .await;
        workspace.update_in(cx, |workspace, window, cx| match found {
            Ok(Some((pdf, boxes))) => reveal_in_pdf(workspace, pdf, boxes, window, cx),
            Ok(None) => show_message(
                workspace,
                "No PDF with SyncTeX data contains this line. Build it with SyncTeX enabled, \
                 for example `latexmk -pdf -synctex=1`."
                    .to_string(),
                cx,
            ),
            Err(error) => show_message(workspace, format!("SyncTeX failed: {error:#}"), cx),
        })
    })
    .detach_and_log_err(cx);
}

/// Scrolls the tab showing `pdf` to `boxes`, opening it beside the editor if needed.
/// Keyboard focus stays where it is.
fn reveal_in_pdf(
    workspace: &mut Workspace,
    pdf: PathBuf,
    boxes: Vec<PdfBox>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    for pane in workspace.panes().to_vec() {
        let view = pane
            .read(cx)
            .items_of_type::<PdfView>()
            .find(|view| view.read(cx).path_on_host(cx) == pdf);
        if let Some(view) = view {
            pane.update(cx, |pane, cx| {
                if let Some(index) = pane.index_for_item(&view) {
                    pane.activate_item(index, false, false, window, cx);
                }
            });
            view.update(cx, |view, cx| view.reveal_boxes(boxes, cx));
            return;
        }
    }

    let focused = window.focused(cx);
    let active_pane = workspace.active_pane().clone();
    let other_pane = workspace
        .panes()
        .iter()
        .find(|pane| **pane != active_pane)
        .cloned();
    let open = match (
        workspace.project().read(cx).find_project_path(&pdf, cx),
        other_pane,
    ) {
        (Some(project_path), Some(pane)) => {
            workspace.open_path(project_path, Some(pane.downgrade()), false, window, cx)
        }
        (Some(project_path), None) => workspace.split_path(project_path, window, cx),
        (None, _) => workspace.open_abs_path(
            pdf,
            OpenOptions {
                visible: Some(OpenVisible::None),
                focus: Some(false),
                ..Default::default()
            },
            window,
            cx,
        ),
    };
    cx.spawn_in(window, async move |_, cx| {
        let item = open.await?;
        cx.update(|window, cx| {
            if let Some(view) = item.downcast::<PdfView>() {
                view.update(cx, |view, cx| view.reveal_boxes(boxes, cx));
            }
            if let Some(focused) = focused {
                window.focus(&focused, cx);
            }
        })
    })
    .detach_and_log_err(cx);
}

/// Opens the result of an inverse search, preferring a pane that already shows the
/// file and otherwise a pane other than the PDF's, then moves the cursor to the line.
pub(crate) fn open_source(
    workspace: &mut Workspace,
    location: Result<Option<SourceLocation>>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let location = match location {
        Ok(Some(location)) => location,
        Ok(None) => {
            show_message(
                workspace,
                "SyncTeX found no source line here. Rebuild the PDF with SyncTeX enabled, \
                 for example `latexmk -pdf -synctex=1`."
                    .to_string(),
                cx,
            );
            return;
        }
        Err(error) => {
            show_message(workspace, format!("SyncTeX failed: {error:#}"), cx);
            return;
        }
    };

    let open = match workspace
        .project()
        .read(cx)
        .find_project_path(&location.path, cx)
    {
        Some(project_path) => {
            let pdf_pane = workspace.active_pane().clone();
            let pane = workspace
                .panes()
                .iter()
                .find(|pane| {
                    pane.read(cx)
                        .item_for_path(project_path.clone(), cx)
                        .is_some()
                })
                .or_else(|| workspace.panes().iter().find(|pane| **pane != pdf_pane))
                .map(|pane| pane.downgrade());
            workspace.open_path(project_path, pane, true, window, cx)
        }
        None => workspace.open_abs_path(
            location.path,
            OpenOptions {
                visible: Some(OpenVisible::None),
                focus: Some(true),
                ..Default::default()
            },
            window,
            cx,
        ),
    };
    let row = location.row;
    cx.spawn_in(window, async move |_, cx| {
        let item = open.await?;
        if let Some(editor) = item.downcast::<Editor>() {
            editor.update_in(cx, |editor, window, cx| {
                editor.go_to_singleton_buffer_point(Point::new(row, 0), window, cx)
            })?;
        }
        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

#[cfg(test)]
mod tests {
    use super::*;

    // Output of `synctex view` for a line that was typeset over two lines of a page.
    const VIEW_OUTPUT: &str = "This is SyncTeX command line utility, version 1.5
SyncTeX result begin
Output:/project/main.pdf
Page:2
x:70.078621
y:511.676422
h:53.999996
v:513.613586
W:245.080933
H:8.855677
before:
offset:-1
middle:
after:
Output:/project/main.pdf
Page:2
x:82.932999
y:523.133423
h:53.999996
v:526.568970
W:245.080933
H:12.682330
before:
offset:-1
middle:
after:
Output:/project/main.pdf
Page:3
x:60.0
y:80.0
h:54.0
v:90.0
W:245.0
H:9.0
before:
offset:-1
middle:
after:
SyncTeX result end
";

    #[test]
    fn parses_view_output() {
        let boxes = parse_view_output(VIEW_OUTPUT);
        // The record on page 3 is dropped: only the first record's page is shown.
        assert_eq!(boxes.len(), 2);
        let first = boxes[0];
        assert_eq!(first.page_index, 1);
        assert!((first.x - 54.).abs() < 0.01);
        assert!((first.y - (513.613586 - 8.855677)).abs() < 0.01);
        assert!((first.width - 245.080933).abs() < 0.01);
        assert!((first.height - 8.855677).abs() < 0.01);
        assert_eq!(boxes[1].page_index, 1);
    }

    #[test]
    fn view_output_without_a_box_uses_the_point() {
        let output = "SyncTeX result begin\nOutput:a.pdf\nPage:1\nx:100\ny:200\nh:0\nv:0\nW:0\nH:0\nSyncTeX result end\n";
        assert_eq!(
            parse_view_output(output),
            vec![PdfBox {
                page_index: 0,
                x: 100. - POINT_BOX_SIZE / 2.,
                y: 200. - POINT_LINE_HEIGHT,
                width: POINT_BOX_SIZE,
                height: POINT_LINE_HEIGHT,
            }]
        );
    }

    #[test]
    fn view_output_with_a_column_box_uses_the_line_of_the_point() {
        // TeX Live 2026's `synctex` reports the whole column for each matching line.
        let output = "SyncTeX result begin\nOutput:/home/user/main.pdf\nPage:2\nx:70.078621\ny:523.133423\nh:53.999996\nv:721.564087\nW:245.080933\nH:669.489380\nSyncTeX result end\n";
        let boxes = parse_view_output(output);
        assert_eq!(boxes.len(), 1);
        let line = boxes[0];
        assert_eq!(line.page_index, 1);
        assert!((line.x - 54.).abs() < 0.01);
        assert!((line.width - 245.080933).abs() < 0.01);
        assert!((line.y - (523.133423 - POINT_LINE_HEIGHT)).abs() < 0.01);
        assert_eq!(line.height, POINT_LINE_HEIGHT);
    }

    #[test]
    fn view_output_with_no_match_is_empty() {
        let output = "This is SyncTeX command line utility, version 1.5\nSyncTeX Warning: No tag for /project/other.tex\n";
        assert!(parse_view_output(output).is_empty());
    }

    #[test]
    fn parses_edit_output() {
        let output = "This is SyncTeX command line utility, version 1.5
SyncTeX result begin
Output:/project/main.pdf
Input:/project/./content/02_husimi.tex
Line:10
Column:-1
Offset:0
Context:
Output:/project/main.pdf
Input:/project/main.tex
Line:42
Column:-1
Offset:0
Context:
SyncTeX result end
";
        assert_eq!(
            parse_edit_output(output),
            Some(SourceLocation {
                path: PathBuf::from("/project/content/02_husimi.tex"),
                row: 9,
            })
        );
        assert_eq!(
            parse_edit_output("SyncTeX result begin\nSyncTeX result end\n"),
            None
        );
    }

    /// Compiles a small document with an included section file and jumps both ways.
    #[test]
    fn round_trips_through_a_compiled_document() {
        let Some(pdflatex) = synctex_executable()
            .ok()
            .and_then(|synctex| Some(synctex.parent()?.join("pdflatex")))
            .filter(|pdflatex| pdflatex.is_file())
        else {
            eprintln!("skipping: pdflatex and synctex aren't installed");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(
            root.join("main.tex"),
            "\\documentclass{article}\n\\begin{document}\nFirst page.\n\\newpage\n\\input{section}\n\\end{document}\n",
        )
        .unwrap();
        let section = root.join("section.tex");
        std::fs::write(
            &section,
            "Intro line.\n\n\\vspace*{10cm}\n\nThe line to find.\n",
        )
        .unwrap();
        let output = futures::executor::block_on(
            util::command::new_command(pdflatex)
                .args(["-synctex=1", "-interaction=batchmode", "main.tex"])
                .current_dir(&root)
                .output(),
        )
        .unwrap();
        assert!(output.status.success());
        let pdf = root.join("main.pdf");
        assert_eq!(
            candidate_pdfs(&section, Vec::new(), &LocalFiles { root: &root }),
            vec![pdf.clone()]
        );

        let run = |command: Result<SyncTexCommand>| {
            futures::executor::block_on(command.unwrap().run()).unwrap()
        };
        // Line 5 (row 4) is on the second page, well below line 1.
        let boxes = parse_view_output(&run(SyncTexCommand::view(None, &section, 4, &pdf)));
        let found = *boxes.first().expect("SyncTeX should find the line");
        assert_eq!(found.page_index, 1);
        let intro = parse_view_output(&run(SyncTexCommand::view(None, &section, 0, &pdf)))[0];
        assert!(found.y > intro.y + 200.);

        let output = run(SyncTexCommand::edit(
            None,
            &pdf,
            found.page_index,
            found.x + 5.,
            found.y + found.height / 2.,
        ));
        assert_eq!(
            parse_edit_output(&output),
            Some(SourceLocation {
                path: section,
                row: 4,
            })
        );
    }

    #[test]
    fn finds_candidate_pdfs() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let content = root.join("content");
        std::fs::create_dir(&content).unwrap();
        let section = content.join("intro.tex");
        std::fs::write(&section, "").unwrap();
        for name in [
            "main.pdf",
            "main.synctex.gz",
            "notes.pdf",
            "notes.synctex.gz",
            "draft.pdf",
        ] {
            std::fs::write(root.join(name), "").unwrap();
        }
        // An open PDF without SyncTeX data is skipped.
        let candidates = candidate_pdfs(
            &section,
            vec![root.join("draft.pdf")],
            &LocalFiles { root: &root },
        );
        let mut names: Vec<_> = candidates
            .iter()
            .map(|pdf| pdf.strip_prefix(&root).unwrap().to_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [PathBuf::from("main.pdf"), PathBuf::from("notes.pdf")]
        );

        // Directories above the worktree root aren't searched.
        assert!(candidate_pdfs(&section, Vec::new(), &LocalFiles { root: &content }).is_empty());
    }
}
