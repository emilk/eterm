use eterm::EguiFrame;
use glium::glutin;

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

    event_loop.run(move |event, _, control_flow| {
        let mut redraw = || {
            let mut new_input = egui_glium.take_raw_input(&display);
            new_input.time = None; // server knows the time
            if last_sent_input.as_ref() != Some(&new_input) {
                client.send_input(new_input.clone());
                last_sent_input = Some(new_input);
            }

            let pixels_per_point = egui_glium.pixels_per_point();
            if let Some(frame) = client.update(pixels_per_point) {
                let EguiFrame {
                    frame_index: _,
                    output,
                    clipped_meshes,
                } = frame;

                egui_glium.handle_output(&display, output);

                {
                    use glium::Surface as _;
                    let mut target = display.draw();

                    let clear_color = egui::Rgba::from_rgb(0.1, 0.3, 0.2);
                    target.clear_color(
                        clear_color[0],
                        clear_color[1],
                        clear_color[2],
                        clear_color[3],
                    );

                    egui_glium.painter_mut().paint_meshes(
                        &display,
                        &mut target,
                        pixels_per_point,
                        clipped_meshes,
                        &client.texture(),
                    );

                    target.finish().unwrap();
                }
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

                display.gl_window().window().request_redraw(); // TODO: ask egui if the events warrants a repaint instead
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
