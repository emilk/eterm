#![forbid(unsafe_code)]
#![warn(
    clippy::all,
    clippy::await_holding_lock,
    clippy::char_lit_as_u8,
    clippy::checked_conversions,
    clippy::dbg_macro,
    clippy::debug_assert_with_mut_call,
    clippy::doc_markdown,
    clippy::empty_enum,
    clippy::enum_glob_use,
    clippy::exit,
    clippy::expl_impl_clone_on_copy,
    clippy::explicit_deref_methods,
    clippy::explicit_into_iter_loop,
    clippy::fallible_impl_from,
    clippy::filter_map_next,
    clippy::float_cmp_const,
    clippy::fn_params_excessive_bools,
    clippy::if_let_mutex,
    clippy::imprecise_flops,
    clippy::inefficient_to_string,
    clippy::invalid_upcast_comparisons,
    clippy::large_types_passed_by_value,
    clippy::let_unit_value,
    clippy::linkedlist,
    clippy::lossy_float_literal,
    clippy::macro_use_imports,
    clippy::manual_ok_or,
    clippy::map_err_ignore,
    clippy::map_flatten,
    clippy::match_on_vec_items,
    clippy::match_same_arms,
    clippy::match_wildcard_for_single_variants,
    clippy::mem_forget,
    clippy::mismatched_target_os,
    clippy::missing_errors_doc,
    clippy::missing_safety_doc,
    clippy::mut_mut,
    clippy::mutex_integer,
    clippy::needless_borrow,
    clippy::needless_continue,
    clippy::needless_pass_by_value,
    clippy::option_option,
    clippy::path_buf_push_overwrite,
    clippy::ptr_as_ptr,
    clippy::ref_option_ref,
    clippy::rest_pat_in_fully_bound_structs,
    clippy::same_functions_in_if_condition,
    clippy::string_add_assign,
    clippy::string_add,
    clippy::string_lit_as_bytes,
    clippy::string_to_string,
    clippy::todo,
    clippy::trait_duplication_in_bounds,
    clippy::unimplemented,
    clippy::unnested_or_patterns,
    clippy::unused_self,
    clippy::useless_transmute,
    clippy::verbose_file_reads,
    clippy::zero_sized_map_values,
    future_incompatible,
    missing_crate_level_docs,
    nonstandard_style,
    rust_2018_idioms
)]
#![allow(clippy::float_cmp)]
#![allow(clippy::manual_range_contains)]

use eterm::EguiFrame;
use glium::glutin;

/// We reserve this much space for eterm to show some stats.
/// The rest is used for the view of the remove server.
const TOP_BAR_HEIGHT: f32 = 24.0;

/// Repaint every so often to check connection status etc.
const MIN_REPAINT_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// eterm viewer viewer.
///
/// Connects to an eterm server somewhere.
#[derive(argh::FromArgs)]
struct Arguments {
    /// which server to connect to, e.g. `127.0.0.1:8505`.
    #[argh(option)]
    url: String,
}

fn main() {
    simple_logger::SimpleLogger::new()
        .with_level(log::LevelFilter::Debug)
        .init()
        .ok();

    let opt: Arguments = argh::from_env();
    let mut client = eterm::Client::new(opt.url);

    let event_loop = glutin::event_loop::EventLoop::with_user_event();
    let display = create_display(&event_loop);

    let mut egui_glium = egui_glium::EguiGlium::new(&display);

    let mut last_sent_input = None;

    let mut latest_eterm_meshes = Default::default();

    let mut needs_repaint = true;
    let mut last_repaint = std::time::Instant::now();

    event_loop.run(move |event, _, control_flow| {
        let mut redraw = || {
            let raw_input = egui_glium.take_raw_input(&display);

            let mut sent_input = raw_input.clone();
            sent_input.time = None; // server knows the time
            if let Some(screen_rect) = &mut sent_input.screen_rect {
                screen_rect.min.y += TOP_BAR_HEIGHT;
                screen_rect.max.y = screen_rect.max.y.max(screen_rect.min.y);
            }

            if last_sent_input.as_ref() != Some(&sent_input) {
                client.send_input(sent_input.clone());
                last_sent_input = Some(sent_input);
                needs_repaint = true;
            }

            let pixels_per_point = egui_glium.pixels_per_point();
            if let Some(frame) = client.update(pixels_per_point) {
                // We got something new from the server!
                let EguiFrame {
                    frame_index: _,
                    output,
                    clipped_meshes,
                } = frame;

                egui_glium.handle_output(&display, output);

                latest_eterm_meshes = clipped_meshes;
                needs_repaint = true;
            }

            if needs_repaint || last_repaint.elapsed() > MIN_REPAINT_INTERVAL {
                needs_repaint = false;
                last_repaint = std::time::Instant::now();

                // paint the eterm viewer ui:
                egui_glium.begin_frame_with_input(raw_input);

                client_gui(egui_glium.ctx(), &client);

                let (needs_repaint_again, clipped_shapes) = egui_glium.end_frame(&display);
                needs_repaint |= needs_repaint_again;

                use glium::Surface as _;
                let mut target = display.draw();

                let cc = egui::Rgba::from_rgb(0.1, 0.3, 0.2);
                target.clear_color(cc[0], cc[1], cc[2], cc[3]);

                egui_glium.painter_mut().paint_meshes(
                    &display,
                    &mut target,
                    pixels_per_point,
                    latest_eterm_meshes.clone(),
                    &client.texture(),
                );

                egui_glium.paint(&display, &mut target, clipped_shapes);

                target.finish().unwrap();
            }

            std::thread::sleep(std::time::Duration::from_millis(10));

            display.gl_window().window().request_redraw();
            *control_flow = glutin::event_loop::ControlFlow::Wait;
        };

        match event {
            // Platform-dependent event handlers to workaround a winit bug
            // See: https://github.com/rust-windowing/winit/issues/987
            // See: https://github.com/rust-windowing/winit/issues/1619
            glutin::event::Event::RedrawEventsCleared if cfg!(windows) => redraw(),
            glutin::event::Event::RedrawRequested(_) if !cfg!(windows) => redraw(),

            glutin::event::Event::WindowEvent { event, .. } => {
                if egui_glium.is_quit_event(&event) {
                    *control_flow = glium::glutin::event_loop::ControlFlow::Exit;
                }

                egui_glium.on_event(&event);

                display.gl_window().window().request_redraw();
            }

            _ => (),
        }
    });
}

fn create_display(event_loop: &glutin::event_loop::EventLoop<()>) -> glium::Display {
    let window_builder = glutin::window::WindowBuilder::new()
        .with_resizable(true)
        .with_inner_size(glutin::dpi::LogicalSize {
            width: 800.0,
            height: 600.0,
        })
        .with_title("eterm viewer");

    let context_builder = glutin::ContextBuilder::new()
        .with_depth_buffer(0)
        .with_double_buffer(Some(true))
        .with_srgb(true)
        .with_stencil_buffer(0)
        .with_vsync(true);

    glium::Display::new(window_builder, context_builder, event_loop).unwrap()
}

fn client_gui(ctx: &egui::CtxRef, client: &eterm::Client) {
    // Chose a theme that sets us apart from the server:
    let mut visuals = ctx.style().visuals.clone();
    let panel_background = if visuals.dark_mode {
        egui::Color32::from_rgb(55, 0, 105)
    } else {
        egui::Color32::from_rgb(255, 240, 0)
    };
    visuals.widgets.noninteractive.bg_fill = panel_background;
    ctx.set_visuals(visuals);

    let height = TOP_BAR_HEIGHT - 4.0; // add some breathing room

    egui::TopBottomPanel::top("eterm_viewer_panel")
        .height_range(height..=height)
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                client_info_bar(ui, client);
            });
        });
}

fn client_info_bar(ui: &mut egui::Ui, client: &eterm::Client) {
    if client.is_connected() {
        ui.label(format!("Connected to {}", client.addr(),));
        ui.separator();
        ui.label(format!("{:.2} MB/s", client.bytes_per_second() * 1e-6));
        ui.separator();
        ui.label(format!(
            "{:5.1} kB / frame",
            client.average_frame_packet_size().unwrap_or(0.0) * 1e-3
        ));
        ui.separator();
        ui.label("adaptive FPS:");
        let fps = client.adaptive_fps().unwrap_or(0.0);
        ui.add_sized(
            [16.0, ui.available_height()],
            egui::Label::new(format!("{:.0}", fps)),
        );
        ui.separator();
        match client.latency() {
            Some(latency) => ui.label(format!("latency: {:.0} ms", latency * 1e3)),
            None => ui.label("latency: "),
        };
    } else {
        ui.label(format!("Connecting to {}…", client.addr()));
    }
}
