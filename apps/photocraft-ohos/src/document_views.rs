//! Regressions for document identity across original close/open/save commands.

use photocraft_engine::Session;
use photocraft_ui_egui::{
    PhotocraftApp, Services,
    state::{DocWindow, View},
};
use serde_json::json;

#[test]
fn original_home_pro_footer_keeps_directory_error_progress_and_success_visible() {
    fn text(shape: &egui::epaint::Shape, result: &mut String) {
        match shape {
            egui::epaint::Shape::Text(shape) => {
                result.push_str(shape.galley.text());
                result.push('\n');
            }
            egui::epaint::Shape::Vec(shapes) => {
                for shape in shapes {
                    text(shape, result);
                }
            }
            _ => {}
        }
    }
    let mut app = PhotocraftApp::new(Session::new(), Services::default());
    app.ui.theme = photocraft_ui_egui::theme::ThemeKind::Pro;
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, app.ui.theme);
    let mut frame = eframe::Frame::_new_kittest();
    for (status, error) in [
        ("Folder permission failed", true),
        ("Publishing 4 processed files…", false),
        ("Published 4 processed files; 0 inputs failed", false),
    ] {
        app.ui.status = status.into();
        app.ui.status_error = error;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1000.0, 800.0),
            )),
            ..Default::default()
        };
        let mut visible = String::new();
        for _ in 0..4 {
            let mut logic_done = false;
            let output = ctx.run_ui(input.clone(), |ui| {
                if !logic_done {
                    eframe::App::logic(&mut app, ui.ctx(), &mut frame);
                    logic_done = true;
                }
                eframe::App::ui(&mut app, ui, &mut frame);
            });
            visible.clear();
            for shape in &output.shapes {
                text(&shape.shape, &mut visible);
            }
            output.drop_without_applying_deltas();
        }
        assert!(
            visible.contains(status),
            "Home must retain the original footer status: {status}"
        );
        assert!(app.session.documents().is_empty());
        assert_eq!(app.ui.status_error, error);
    }
}

#[test]
fn closing_first_and_middle_documents_keeps_each_survivors_camera_layer_and_window()
-> Result<(), String> {
    for closed in [0, 1] {
        let mut app = PhotocraftApp::new(Session::new(), Services::default());
        for (name, color) in [
            ("First.psd", "#ff0000"),
            ("Second.psd", "#00ff00"),
            ("Third.psd", "#0000ff"),
        ] {
            app.run(
                "file.new",
                json!({"width":8,"height":8,"name":name,"background":color}),
            )?;
            app.run(
                "layer.new.layer",
                json!({"name":format!("{name} selected layer")}),
            )?;
        }
        let documents = app.session.documents().to_vec();
        let views: Vec<_> = (0..3)
            .map(|i| View {
                zoom: [0.2, 0.439, 1.5][i],
                center: [2.0 + i as f32, 5.0 - i as f32],
                fit_pending: false,
                doc_size: [8, 8],
            })
            .collect();
        app.ui.views = views.clone();
        app.ui.windows = (0..3)
            .map(|i| DocWindow {
                id: 100 + i as u64,
                document: i,
                view: views[i].clone(),
                open: true,
            })
            .collect();
        app.run("file.close", json!({"document":closed}))?;
        let survivors: Vec<_> = (0..3).filter(|i| *i != closed).collect();
        for (index, old) in survivors.iter().copied().enumerate() {
            let state = &app.session.documents()[index];
            assert_eq!(
                state.doc, documents[old].doc,
                "closing a tab must preserve the real document"
            );
            assert_eq!(state.active_layer, documents[old].active_layer);
            assert_eq!(app.ui.views[index], views[old]);
            let window = app
                .ui
                .windows
                .iter()
                .find(|window| window.id == 100 + old as u64)
                .ok_or("surviving window missing")?;
            assert_eq!(window.document, index);
            assert_eq!(window.view, views[old]);
        }
        assert!(
            !app.ui
                .windows
                .iter()
                .any(|window| window.id == 100 + closed as u64)
        );
        let bytes =
            photocraft_io::export(&documents[closed].doc, "reopened.psd", &Default::default())
                .map_err(|error| error.to_string())?
                .bytes;
        let imported = photocraft_io::import("reopened.psd", &bytes)
            .map_err(|error| error.to_string())?
            .document;
        let new_index = app
            .session
            .add_document(imported, Some("reopened.psd".into()));
        app.sync_views();
        assert_eq!(new_index, 2);
        assert_eq!(
            app.ui.views[2],
            View::default(),
            "a reopened PSD receives its own initial camera"
        );
        assert_eq!(
            app.ui.windows.len(),
            2,
            "reopening must not resurrect a closed extra window"
        );
        for (index, old) in survivors.iter().copied().enumerate() {
            assert_eq!(app.ui.views[index], views[old]);
            assert_eq!(
                app.session.documents()[index].active_layer,
                documents[old].active_layer
            );
            let document = &app.session.documents()[index].doc;
            assert_eq!(
                document.layers[0]
                    .surface()
                    .ok_or("background missing")?
                    .rgba(0, 0),
                documents[old].doc.layers[0]
                    .surface()
                    .ok_or("original background missing")?
                    .rgba(0, 0)
            );
        }
    }
    Ok(())
}

#[test]
fn close_and_open_before_repaint_does_not_reuse_a_removed_documents_camera() -> Result<(), String> {
    let mut app = PhotocraftApp::new(Session::new(), Services::default());
    for name in ["first", "middle", "last"] {
        app.run("file.new", json!({"width":4,"height":4,"name":name}))?;
    }
    app.ui.views[0].zoom = 0.2;
    app.ui.views[1].zoom = 0.439;
    app.ui.views[2].zoom = 1.5;
    app.session.close(1);
    app.session
        .execute(
            "file.new",
            json!({"width":4,"height":4,"name":"replacement"}),
        )
        .map_err(|error| error.to_string())?;
    app.sync_views();
    assert_eq!(app.ui.views[0].zoom, 0.2);
    assert_eq!(app.ui.views[1].zoom, 1.5);
    assert_eq!(app.ui.views[2], View::default());
    assert_eq!(app.session.documents()[1].doc.name, "last");
    assert_eq!(app.session.documents()[2].doc.name, "replacement");
    Ok(())
}

#[test]
fn original_pro_document_strip_keeps_active_long_tab_visible_and_overflow_switches_every_document()
-> Result<(), String> {
    fn shapes(
        shape: &egui::epaint::Shape,
        clip: egui::Rect,
        text: &mut Vec<(String, egui::Rect)>,
        strips: &mut Vec<egui::Rect>,
        color: egui::Color32,
    ) {
        match shape {
            egui::epaint::Shape::Text(value) => text.push((
                value.galley.text().into(),
                egui::Rect::from_min_size(value.pos, value.galley.size()).intersect(clip),
            )),
            egui::epaint::Shape::Rect(value)
                if value.fill == color && (value.rect.height() - 26.0).abs() < 0.1 =>
            {
                strips.push(value.rect.intersect(clip))
            }
            egui::epaint::Shape::Vec(values) => {
                for value in values {
                    shapes(value, clip, text, strips, color);
                }
            }
            _ => {}
        }
    }
    let mut app = PhotocraftApp::new(Session::new(), Services::default());
    app.ui.theme = photocraft_ui_egui::theme::ThemeKind::Pro;
    let names = [
        "First-PhotoCraft-Long-Document-20261008.psd",
        "Second-PhotoCraft-Long-Document-20261008.psd",
        "Third-PhotoCraft-Long-Document-20261008.psd",
    ];
    for name in names {
        app.run("file.new", json!({"width":8,"height":8,"name":name}))?;
    }
    let ids: Vec<_> = app
        .session
        .documents()
        .iter()
        .map(|document| document.doc.id)
        .collect();
    let ctx = egui::Context::default();
    PhotocraftApp::setup_context(&ctx, app.ui.theme);
    let mut frame = eframe::Frame::_new_kittest();
    let mut render = |app: &mut PhotocraftApp, events: Vec<egui::Event>| {
        let output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(640.0, 700.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                eframe::App::logic(app, ui.ctx(), &mut frame);
                eframe::App::ui(app, ui, &mut frame);
            },
        );
        let (mut text, mut strips) = (Vec::new(), Vec::new());
        let color = photocraft_ui_egui::theme::Tokens::get(&ctx).tab_strip;
        for shape in &output.shapes {
            shapes(&shape.shape, shape.clip_rect, &mut text, &mut strips, color);
        }
        output.drop_without_applying_deltas();
        (text, strips)
    };
    let mut click = |app: &mut PhotocraftApp, point: egui::Pos2| {
        for pressed in [true, false] {
            let _ = render(
                app,
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    },
                ],
            );
        }
        render(app, Vec::new())
    };
    // Render a narrow real app, open the original strip overflow, and activate
    // each hidden document through an actual menu-button click.
    for target in [0, 1, 2] {
        let mut output = click(&mut app, egui::pos2(5.0, 695.0));
        for _ in 0..3 {
            output = click(&mut app, egui::pos2(5.0, 695.0));
        }
        let active = app.session.active_index().ok_or("active missing")?;
        let label = output
            .0
            .iter()
            .find(|(name, _)| name.starts_with(names[active].split('-').next().unwrap_or_default()))
            .ok_or("active tab text missing")?;
        let strip = output
            .1
            .iter()
            .filter(|strip| strip.contains(label.1.center()))
            .max_by(|a, b| a.width().total_cmp(&b.width()))
            .copied()
            .ok_or("document strip missing")?;
        assert!(strip.contains(label.1.center()));
        assert!(label.1.width() > 15.0);
        if active == target {
            continue;
        }
        let menu = click(&mut app, egui::pos2(strip.right() - 10.0, strip.center().y));
        let item = menu
            .0
            .iter()
            .find(|(name, _)| name.starts_with(names[target]))
            .ok_or("overflow document missing")?;
        let point = item.1.center();
        let _ = click(&mut app, point);
        assert_eq!(
            app.session.active().ok_or("active missing")?.doc.id,
            ids[target]
        );
    }
    Ok(())
}
