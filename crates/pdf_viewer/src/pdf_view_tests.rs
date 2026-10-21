use std::{path::Path, sync::Arc, time::Duration};

use client::{Client, UserStore};
use clock::FakeSystemClock;
use extension::ExtensionHostProxy;
use fs::{FakeFs, Fs as _};
use futures::StreamExt as _;
use gpui::{Entity, TestAppContext, VisualTestContext, point, px, size};
use http_client::{BlockedHttpClient, FakeHttpClient};
use language::LanguageRegistry;
use node_runtime::NodeRuntime;
use project::{Project, ProjectPath};
use remote::RemoteClient;
use remote_server::{HeadlessAppState, HeadlessProject};
use settings::SettingsStore;
use util::rel_path::rel_path;

use super::*;
use crate::layout::{CONTENT_PADDING, ZoomMode};
use crate::pdf_item::PdfItemEvent;
use crate::test_support::{linked_pdf, multi_page_pdf};

fn init_test(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let settings_store = SettingsStore::test(cx);
        cx.set_global(settings_store);
        theme_settings::init(theme::LoadThemes::JustBase, cx);
    });
}

/// Opens a generated PDF in a `PdfView`, or returns `None` when Pdfium isn't installed.
async fn open_pdf_view(
    pages: usize,
    cx: &mut TestAppContext,
) -> Option<(Entity<PdfView>, &mut VisualTestContext)> {
    open_pdf_bytes(multi_page_pdf(pages), cx).await
}

/// Opens the given PDF in a `PdfView`, or returns `None` when Pdfium isn't installed.
async fn open_pdf_bytes(
    bytes: Vec<u8>,
    cx: &mut TestAppContext,
) -> Option<(Entity<PdfView>, &mut VisualTestContext)> {
    if let Err(error) = kkpdf_zed::PdfWorker::global() {
        eprintln!("skipping: {error}");
        return None;
    }
    init_test(cx);
    // The Pdfium worker is a real thread, so let the executor wait for it.
    cx.executor().allow_parking();

    let fs = FakeFs::new(cx.executor());
    fs.create_dir(Path::new("/root")).await.unwrap();
    fs.insert_file("/root/doc.pdf", bytes).await;
    let project = Project::test(fs, [Path::new("/root")], cx).await;
    let worktree_id = cx.update(|cx| project.read(cx).worktrees(cx).next().unwrap().read(cx).id());
    let path = ProjectPath {
        worktree_id,
        path: rel_path("doc.pdf").into(),
    };
    let pdf_item = cx
        .update(|cx| <PdfItem as project::ProjectItem>::try_open(&project, &path, cx))
        .expect("PDF files should be claimed by the PDF viewer")
        .await
        .expect("PDF should open");

    let (view, cx) =
        cx.add_window_view(|window, cx| PdfView::new(pdf_item, project.clone(), window, cx));
    draw(cx);
    Some((view, cx))
}

fn draw(cx: &mut VisualTestContext) {
    cx.run_until_parked();
    cx.update(|window, cx| {
        window.refresh();
        window.draw(cx).clear(cx);
    });
}

/// Draws until every visible tile has been rendered by the worker.
fn draw_until_rendered(view: &Entity<PdfView>, cx: &mut VisualTestContext) {
    for _ in 0..200 {
        draw(cx);
        if cx.read(|cx| view.read(cx).tiles.pending.is_empty()) {
            draw(cx);
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("tiles never finished rendering");
}

#[gpui::test]
async fn opens_a_pdf_opened_on_its_own(cx: &mut TestAppContext) {
    if let Err(error) = kkpdf_zed::PdfWorker::global() {
        eprintln!("skipping: {error}");
        return;
    }
    init_test(cx);
    cx.executor().allow_parking();

    // Opening a single file (Finder, `zed file.pdf`) makes the file itself the
    // worktree root, so its path inside the worktree is empty.
    let fs = FakeFs::new(cx.executor());
    fs.create_dir(Path::new("/root")).await.unwrap();
    fs.insert_file("/root/Lecture Notes.PDF", multi_page_pdf(2))
        .await;
    let project = Project::test(fs, [Path::new("/root/Lecture Notes.PDF")], cx).await;
    let worktree_id = cx.update(|cx| project.read(cx).worktrees(cx).next().unwrap().read(cx).id());
    let path = ProjectPath {
        worktree_id,
        path: util::rel_path::RelPath::empty_arc(),
    };

    let pdf_item = cx
        .update(|cx| <PdfItem as project::ProjectItem>::try_open(&project, &path, cx))
        .expect("a lone PDF should be claimed by the PDF viewer")
        .await
        .expect("a lone PDF should open");
    cx.read(|cx| {
        let pdf_item = pdf_item.read(cx);
        assert_eq!(pdf_item.document().pages.len(), 2);
        assert_eq!(
            pdf_item.path_on_host(cx),
            Path::new("/root/Lecture Notes.PDF")
        );
    });
}

#[gpui::test]
async fn opens_at_the_top_of_the_first_page(cx: &mut TestAppContext) {
    let Some((view, cx)) = open_pdf_view(12, cx).await else {
        return;
    };
    draw_until_rendered(&view, cx);
    // Windows usually settle into their final size over a few frames.
    for window_size in [size(px(300.), px(200.)), size(px(1400.), px(900.))] {
        cx.simulate_resize(window_size);
        draw_until_rendered(&view, cx);
    }
    cx.read(|cx| {
        let view = view.read(cx);
        assert_eq!(view.scroll.scroll_position().y, px(0.));
        assert_eq!(view.current_page(), 0);
    });
}

#[gpui::test]
async fn zooming_keeps_the_point_under_the_cursor(cx: &mut TestAppContext) {
    let Some((view, cx)) = open_pdf_view(5, cx).await else {
        return;
    };
    cx.simulate_resize(size(px(1000.), px(800.)));
    draw_until_rendered(&view, cx);
    view.update(cx, |view, cx| view.go_to_page(2, cx));
    draw(cx);

    let (viewport, anchor_before) = cx.read(|cx| {
        let view = view.read(cx);
        let viewport = view.scroll.viewport_bounds();
        let cursor = viewport.origin + point(px(300.), px(250.));
        let content = view.scroll.scroll_position() + (cursor - viewport.origin);
        (
            viewport,
            view.layout.as_ref().unwrap().anchor_at(content).unwrap(),
        )
    });
    let cursor = viewport.origin + point(px(300.), px(250.));
    view.update(cx, |view, cx| {
        view.set_zoom_mode(ZoomMode::Fixed(2.5), Some(cursor), cx)
    });
    draw_until_rendered(&view, cx);

    cx.read(|cx| {
        let view = view.read(cx);
        let layout = view.layout.as_ref().unwrap();
        assert_eq!(layout.zoom, 2.5);
        let content = view.scroll.scroll_position() + (cursor - viewport.origin);
        let anchor_after = layout.anchor_at(content).unwrap();
        assert_eq!(anchor_after.page_index, anchor_before.page_index);
        assert!((anchor_after.x - anchor_before.x).abs() < 1.);
        assert!((anchor_after.y - anchor_before.y).abs() < 1.);
    });
}

#[gpui::test]
async fn page_navigation(cx: &mut TestAppContext) {
    let Some((view, cx)) = open_pdf_view(8, cx).await else {
        return;
    };
    cx.simulate_resize(size(px(900.), px(700.)));
    draw_until_rendered(&view, cx);

    view.update(cx, |view, cx| view.go_to_page(5, cx));
    draw(cx);
    cx.read(|cx| {
        let view = view.read(cx);
        let page_top = view.layout.as_ref().unwrap().pages[5].top();
        assert_eq!(view.scroll.scroll_position().y, page_top - PAGE_GAP / 2.);
    });

    // Jumping past the end clamps to the last page.
    view.update(cx, |view, cx| view.go_to_page(100, cx));
    draw(cx);
    cx.read(|cx| assert_eq!(view.read(cx).current_page(), 7));

    cx.focus(&view);
    cx.dispatch_action(FirstPage);
    draw(cx);
    cx.read(|cx| assert_eq!(view.read(cx).scroll.scroll_position().y, px(0.)));
}

#[gpui::test]
async fn renders_visible_tiles_and_paints_them(cx: &mut TestAppContext) {
    let Some((view, cx)) = open_pdf_view(3, cx).await else {
        return;
    };
    cx.simulate_resize(size(px(800.), px(600.)));
    draw_until_rendered(&view, cx);
    cx.read(|cx| {
        let view = view.read(cx);
        assert!(view.tiles.failed.is_empty(), "no tile should fail");
        assert!(
            !view.tiles.tiles.is_empty(),
            "visible tiles should be cached"
        );
        let layout = view.layout.as_ref().unwrap();
        assert_eq!(layout.pages[0].top(), CONTENT_PADDING);
    });
}

#[gpui::test]
async fn moving_the_mouse_without_a_button_does_not_pan(cx: &mut TestAppContext) {
    let Some((view, cx)) = open_pdf_view(6, cx).await else {
        return;
    };
    cx.simulate_resize(size(px(900.), px(700.)));
    draw_until_rendered(&view, cx);
    let center = cx.read(|cx| view.read(cx).scroll.viewport_bounds().center());

    // Hovering never scrolls.
    cx.simulate_mouse_move(center, None, gpui::Modifiers::none());
    cx.simulate_mouse_move(
        center - point(px(0.), px(200.)),
        None,
        gpui::Modifiers::none(),
    );
    draw(cx);
    cx.read(|cx| {
        assert_eq!(
            view.read(cx).scroll.scroll_position(),
            point(px(0.), px(0.))
        )
    });

    // Dragging with the button held pans the document.
    cx.simulate_mouse_down(center, gpui::MouseButton::Left, gpui::Modifiers::none());
    cx.simulate_mouse_move(
        center - point(px(0.), px(150.)),
        Some(gpui::MouseButton::Left),
        gpui::Modifiers::none(),
    );
    draw(cx);
    cx.read(|cx| assert_eq!(view.read(cx).scroll.scroll_position().y, px(150.)));

    // If the mouse-up is never delivered, the next plain move ends the drag.
    cx.simulate_mouse_move(
        center - point(px(0.), px(300.)),
        None,
        gpui::Modifiers::none(),
    );
    cx.simulate_mouse_move(center, None, gpui::Modifiers::none());
    draw(cx);
    cx.read(|cx| {
        let view = view.read(cx);
        assert_eq!(view.scroll.scroll_position().y, px(150.));
        assert!(view.drag_position.is_none());
    });
}

/// A project connected to a fake remote host whose files live in `server_fs`,
/// set up the way upstream's remote editing tests do it.
async fn remote_project(
    server_fs: &Arc<FakeFs>,
    cx: &mut TestAppContext,
    server_cx: &mut TestAppContext,
) -> Entity<Project> {
    cx.update(|cx| release_channel::init(semver::Version::new(0, 0, 0), cx));
    server_cx.update(|cx| release_channel::init(semver::Version::new(0, 0, 0), cx));

    let (options, server_session, _) = RemoteClient::fake_server(cx, server_cx);
    server_cx.update(HeadlessProject::init);
    let headless = server_cx.new(|cx| {
        HeadlessProject::new(
            HeadlessAppState {
                session: server_session,
                fs: server_fs.clone(),
                http_client: Arc::new(BlockedHttpClient),
                node_runtime: NodeRuntime::unavailable(),
                languages: Arc::new(LanguageRegistry::new(cx.background_executor().clone())),
                extension_host_proxy: Arc::new(ExtensionHostProxy::new()),
                startup_time: std::time::Instant::now(),
            },
            false,
            cx,
        )
    });
    let remote_client = RemoteClient::connect_mock(options, cx).await;

    let client = cx.update(|cx| {
        Client::new(
            Arc::new(FakeSystemClock::new()),
            FakeHttpClient::with_404_response(),
            cx,
        )
    });
    let user_store = cx.new(|cx| UserStore::new(client.clone(), cx));
    let languages = Arc::new(LanguageRegistry::test(cx.executor()));
    let client_fs = FakeFs::new(cx.executor());
    cx.update(|cx| Project::init(&client, cx));
    let project = cx.update(|cx| {
        Project::remote(
            remote_client,
            client,
            NodeRuntime::unavailable(),
            user_store,
            languages,
            client_fs,
            false,
            cx,
        )
    });
    project
        .update(cx, |_, cx| cx.on_release(move |_, _| drop(headless)))
        .detach();
    project
}

#[gpui::test]
async fn opens_and_reloads_a_pdf_in_a_remote_project(
    cx: &mut TestAppContext,
    server_cx: &mut TestAppContext,
) {
    if let Err(error) = kkpdf_zed::PdfWorker::global() {
        eprintln!("skipping: {error}");
        return;
    }
    init_test(cx);

    let server_fs = FakeFs::new(server_cx.executor());
    server_fs.create_dir(Path::new("/code")).await.unwrap();
    server_fs
        .insert_file("/code/doc.pdf", multi_page_pdf(2))
        .await;
    let project = remote_project(&server_fs, cx, server_cx).await;
    let (worktree, _) = project
        .update(cx, |project, cx| {
            project.find_or_create_worktree("/code", true, cx)
        })
        .await
        .unwrap();
    cx.executor().run_until_parked();
    assert!(cx.read(|cx| worktree.read(cx).is_remote()));

    // From here on, real threads (the Pdfium worker and the download's file
    // write) are involved, so let the executor wait for them in real time.
    cx.executor().allow_parking();
    let path = ProjectPath {
        worktree_id: cx.read(|cx| worktree.read(cx).id()),
        path: rel_path("doc.pdf").into(),
    };
    let pdf_item = cx
        .update(|cx| <PdfItem as project::ProjectItem>::try_open(&project, &path, cx))
        .expect("PDF files should be claimed by the PDF viewer")
        .await
        .expect("a PDF in a remote project should open");
    cx.read(|cx| {
        let pdf_item = pdf_item.read(cx);
        assert_eq!(pdf_item.document().pages.len(), 2);
        assert!(!pdf_item.file().is_local);
        assert_eq!(pdf_item.abs_path(cx), None);
    });

    let (reloaded_tx, mut reloaded_rx) = futures::channel::mpsc::unbounded();
    cx.update(|cx| {
        cx.subscribe(&pdf_item, move |_, event, _| {
            if matches!(event, PdfItemEvent::Reloaded) {
                reloaded_tx.unbounded_send(()).ok();
            }
        })
        .detach();
    });
    server_fs
        .insert_file("/code/doc.pdf", multi_page_pdf(5))
        .await;
    reloaded_rx
        .next()
        .await
        .expect("the PDF should reload after it changed on the remote host");
    cx.read(|cx| {
        let pdf_item = pdf_item.read(cx);
        assert_eq!(pdf_item.document().pages.len(), 5);
        assert_eq!(pdf_item.generation(), 1);
        assert!(!pdf_item.file().is_local);
    });

    let (view, cx) =
        cx.add_window_view(|window, cx| PdfView::new(pdf_item, project.clone(), window, cx));
    let tooltip = cx.read(|cx| view.read(cx).tab_tooltip_text(cx));
    assert_eq!(tooltip.as_deref(), Some("code/doc.pdf"));
}

#[gpui::test]
async fn an_empty_pdf_in_a_remote_project_reports_an_error(
    cx: &mut TestAppContext,
    server_cx: &mut TestAppContext,
) {
    init_test(cx);
    let server_fs = FakeFs::new(server_cx.executor());
    server_fs.create_dir(Path::new("/code")).await.unwrap();
    server_fs.insert_file("/code/empty.pdf", Vec::new()).await;
    let project = remote_project(&server_fs, cx, server_cx).await;
    let (worktree, _) = project
        .update(cx, |project, cx| {
            project.find_or_create_worktree("/code", true, cx)
        })
        .await
        .unwrap();
    cx.executor().run_until_parked();

    let path = ProjectPath {
        worktree_id: cx.read(|cx| worktree.read(cx).id()),
        path: rel_path("empty.pdf").into(),
    };
    let error = cx
        .update(|cx| <PdfItem as project::ProjectItem>::try_open(&project, &path, cx))
        .expect("PDF files should be claimed by the PDF viewer")
        .await
        .err()
        .expect("an empty file should not open");
    assert_eq!(error.to_string(), "empty.pdf is empty");
}

#[gpui::test]
async fn reopens_a_pdf_in_a_remote_project_from_its_saved_path(
    cx: &mut TestAppContext,
    server_cx: &mut TestAppContext,
) {
    if let Err(error) = kkpdf_zed::PdfWorker::global() {
        eprintln!("skipping: {error}");
        return;
    }
    init_test(cx);

    let server_fs = FakeFs::new(server_cx.executor());
    server_fs.create_dir(Path::new("/code/docs")).await.unwrap();
    server_fs
        .insert_file("/code/docs/doc.pdf", multi_page_pdf(3))
        .await;
    let project = remote_project(&server_fs, cx, server_cx).await;
    let (worktree, _) = project
        .update(cx, |project, cx| {
            project.find_or_create_worktree("/code", true, cx)
        })
        .await
        .unwrap();
    cx.executor().run_until_parked();

    cx.executor().allow_parking();
    let path = ProjectPath {
        worktree_id: cx.read(|cx| worktree.read(cx).id()),
        path: rel_path("docs/doc.pdf").into(),
    };
    let pdf_item = cx
        .update(|cx| <PdfItem as project::ProjectItem>::try_open(&project, &path, cx))
        .expect("PDF files should be claimed by the PDF viewer")
        .await
        .expect("a PDF in a remote project should open");

    // Session restore saves this path, so it must name the file on the host.
    let saved_path = cx.read(|cx| pdf_item.read(cx).path_on_host(cx));
    assert_eq!(saved_path, Path::new("/code/docs/doc.pdf"));

    let restored = cx
        .spawn({
            let project = project.clone();
            async move |mut cx| PdfItem::open_abs_path(project, saved_path, &mut cx).await
        })
        .await
        .expect("the saved path should open again");
    cx.read(|cx| {
        let restored = restored.read(cx);
        assert_eq!(restored.document().pages.len(), 3);
        assert_eq!(restored.project_path(cx), path);
        assert_eq!(project.read(cx).worktrees(cx).count(), 1);
    });
}

#[gpui::test]
async fn revealing_a_synctex_box_scrolls_to_it_and_highlights_it(cx: &mut TestAppContext) {
    let Some((view, cx)) = open_pdf_view(8, cx).await else {
        return;
    };
    cx.simulate_resize(size(px(900.), px(700.)));
    draw_until_rendered(&view, cx);

    let pdf_box = PdfBox {
        page_index: 5,
        x: 72.,
        y: 300.,
        width: 200.,
        height: 10.,
    };
    view.update(cx, |view, cx| view.reveal_boxes(vec![pdf_box], cx));
    draw(cx);
    let scroll_position = cx.read(|cx| {
        let view = view.read(cx);
        let layout = view.layout.as_ref().unwrap();
        let position = layout
            .position_of(PageAnchor {
                page_index: 5,
                x: 72.,
                y: 300.,
            })
            .unwrap();
        // The box sits a third of the way down the view.
        let offset = position.y - view.scroll.scroll_position().y;
        assert!((offset - view.scroll.viewport_bounds().size.height / 3.).abs() < px(1.));
        assert_eq!(view.highlight, vec![pdf_box]);
        view.scroll.scroll_position()
    });

    // A box that is already on screen is highlighted without scrolling.
    let nearby = PdfBox { y: 330., ..pdf_box };
    view.update(cx, |view, cx| view.reveal_boxes(vec![nearby], cx));
    draw(cx);
    cx.read(|cx| {
        let view = view.read(cx);
        assert_eq!(view.scroll.scroll_position(), scroll_position);
        assert_eq!(view.highlight, vec![nearby]);
    });

    cx.executor().advance_clock(HIGHLIGHT_DURATION);
    draw(cx);
    cx.read(|cx| assert!(view.read(cx).highlight.is_empty()));
}

#[gpui::test]
async fn maps_a_click_to_a_point_on_the_page(cx: &mut TestAppContext) {
    let Some((view, cx)) = open_pdf_view(4, cx).await else {
        return;
    };
    cx.simulate_resize(size(px(900.), px(700.)));
    draw_until_rendered(&view, cx);
    view.update(cx, |view, cx| view.go_to_page(2, cx));
    draw(cx);

    cx.read(|cx| {
        let view = view.read(cx);
        let layout = view.layout.as_ref().unwrap();
        let viewport = view.scroll.viewport_bounds();
        let page = layout.pages[2];
        // 30 points right of and 40 points below the top-left corner of page 3.
        let click = viewport.origin - view.scroll.scroll_position()
            + page.origin
            + point(px(30. * layout.zoom), px(40. * layout.zoom));
        let anchor = view.page_point_at(click).unwrap();
        assert_eq!(anchor.page_index, 2);
        assert!((anchor.x - 30.).abs() < 0.01);
        assert!((anchor.y - 40.).abs() < 0.01);
    });
}

#[gpui::test]
async fn reads_links_and_their_targets(cx: &mut TestAppContext) {
    let Ok(worker) = kkpdf_zed::PdfWorker::global() else {
        eprintln!("skipping: Pdfium isn't installed");
        return;
    };
    cx.executor().allow_parking();
    let (tx, rx) = futures::channel::oneshot::channel();
    worker.open(linked_pdf(), move |result| {
        tx.send(result).ok();
    });
    let document = rx.await.unwrap().unwrap();
    let links_on = |page_index| {
        let (tx, rx) = futures::channel::oneshot::channel();
        worker.links(document.id, page_index, move |result| {
            tx.send(result).ok();
        });
        rx
    };
    let close = |a: f32, b: f32| (a - b).abs() < 0.1;

    let mut links = links_on(0).await.unwrap().unwrap();
    links.sort_by(|a, b| a.y.total_cmp(&b.y));
    assert_eq!(links.len(), 3);
    // A named destination, as `\eqref` writes it: page 2, 72 points from the left
    // and 400 points up, so 792 - 400 = 392 points from the top.
    assert!(close(links[0].x, 100.) && close(links[0].y, 172.));
    assert!(close(links[0].width, 100.) && close(links[0].height, 20.));
    let LinkTarget::Page { page_index, x, y } = links[0].target else {
        panic!(
            "expected a link into the document, got {:?}",
            links[0].target
        );
    };
    assert_eq!(page_index, 1);
    assert!(close(x.unwrap(), 72.) && close(y.unwrap(), 392.));
    // Fit width at y = 300 on the last page, whose media box starts at y = 100.
    assert!(close(links[1].y, 272.));
    let LinkTarget::Page { page_index, x, y } = links[1].target else {
        panic!(
            "expected a link into the document, got {:?}",
            links[1].target
        );
    };
    assert_eq!((page_index, x), (2, None));
    assert!(close(y.unwrap(), 892. - 300.));
    assert_eq!(
        links[2].target,
        LinkTarget::Uri("https://example.com/".into())
    );

    // Links on a page with an offset media box are placed relative to its top too.
    let links = links_on(2).await.unwrap().unwrap();
    assert_eq!(links.len(), 1);
    assert!(close(links[0].x, 100.) && close(links[0].y, 892. - 720.));
    assert_eq!(
        links[0].target,
        LinkTarget::Page {
            page_index: 0,
            x: None,
            y: None
        }
    );
    worker.close(document.id);
}

#[gpui::test]
async fn clicking_a_link_scrolls_to_its_target_and_going_back_returns(cx: &mut TestAppContext) {
    let Some((view, cx)) = open_pdf_bytes(linked_pdf(), cx).await else {
        return;
    };
    cx.simulate_resize(size(px(900.), px(700.)));
    draw_until_rendered(&view, cx);
    for _ in 0..200 {
        draw(cx);
        let loaded = cx.read(|cx| {
            matches!(
                view.read(cx).links.pages.get(&0),
                Some(PageLinksState::Loaded(_))
            )
        });
        if loaded {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    // The window position of a point given in PDF points on the first page.
    let window_point = |cx: &mut VisualTestContext, x: f32, y: f32| {
        cx.read(|cx| {
            let view = view.read(cx);
            let layout = view.layout.as_ref().unwrap();
            view.scroll.viewport_bounds().origin - view.scroll.scroll_position()
                + layout.pages[0].origin
                + point(px(x * layout.zoom), px(y * layout.zoom))
        })
    };
    let before = cx.read(|cx| view.read(cx).top_anchor().unwrap());
    assert_eq!(before.page_index, 0);

    // Hovering a link shows a pointing hand.
    let on_link = window_point(cx, 150., 182.);
    cx.simulate_mouse_move(on_link, None, gpui::Modifiers::none());
    cx.read(|cx| assert!(view.read(cx).hovering_link));

    // Clicking the equation link puts its target, page 2 at 392 points, at the top.
    cx.simulate_click(on_link, gpui::Modifiers::none());
    draw(cx);
    cx.read(|cx| {
        let view = view.read(cx);
        let anchor = view.top_anchor().unwrap();
        assert_eq!(anchor.page_index, 1);
        assert!((anchor.y - 392.).abs() < 1., "scrolled to {anchor:?}");
    });

    // Going back returns to where the view was before the click.
    view.update_in(cx, |view, window, cx| {
        assert!(view.navigate(Arc::new(before), window, cx));
    });
    draw(cx);
    cx.read(|cx| {
        let anchor = view.read(cx).top_anchor().unwrap();
        assert_eq!(anchor.page_index, before.page_index);
        assert!((anchor.y - before.y).abs() < 1.);
    });
}
