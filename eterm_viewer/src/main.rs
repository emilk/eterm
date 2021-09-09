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

    event_loop.run(move |event, _, control_flow| {
        let mut redraw = || {
            client.send_input(egui_glium.take_raw_input(&display));

            let pixels_per_point = egui_glium.pixels_per_point();
            let frame = client.update(pixels_per_point);

            let EguiFrame {
                frame_index,
                output,
                clipped_meshes,
            } = frame;

            if output.needs_repaint {
                // eprintln!("painted frame {}, needs_repaint", frame_index);
                display.gl_window().window().request_redraw();
                glutin::event_loop::ControlFlow::Poll
            } else {
                // eprintln!("painted frame {}", frame_index);
                glutin::event_loop::ControlFlow::Wait
            };

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
        .with_srgb(true)
        .with_stencil_buffer(0)
        .with_vsync(true);

    glium::Display::new(window_builder, context_builder, event_loop).unwrap()
}
