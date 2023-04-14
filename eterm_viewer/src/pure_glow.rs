//! Using glow to render the server received egui primitives.

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release
#![allow(unsafe_code)]

use egui::{Pos2, RawInput};
use egui_glow::{Painter, ShaderVersion};
use egui_winit::winit;

pub use egui_winit::EventResponse;

use crate::Arguments;

/// The majority of `GlutinWindowContext` is taken from `eframe`
struct GlutinWindowContext {
    window: winit::window::Window,
    gl_context: glutin::context::PossiblyCurrentContext,
    gl_display: glutin::display::Display,
    gl_surface: glutin::surface::Surface<glutin::surface::WindowSurface>,
}

impl GlutinWindowContext {
    // refactor this function to use `glutin-winit` crate eventually.
    // preferably add android support at the same time.
    #[allow(unsafe_code)]
    unsafe fn new(event_loop: &winit::event_loop::EventLoopWindowTarget<()>) -> Self {
        use egui::NumExt;
        use glutin::context::NotCurrentGlContextSurfaceAccessor;
        use glutin::display::GetGlDisplay;
        use glutin::display::GlDisplay;
        use glutin::prelude::GlSurface;
        use raw_window_handle::HasRawWindowHandle;
        let winit_window_builder = winit::window::WindowBuilder::new()
            .with_resizable(true)
            .with_inner_size(winit::dpi::LogicalSize {
                width: 800.0,
                height: 600.0,
            })
            .with_title("egui eterm client") // Keep hidden until we've painted something. See https://github.com/emilk/egui/pull/2279
            .with_visible(false);

        let config_template_builder = glutin::config::ConfigTemplateBuilder::new()
            .prefer_hardware_accelerated(None)
            .with_depth_size(0)
            .with_stencil_size(0)
            .with_transparency(false);

        tracing::debug!("trying to get gl_config");
        let (mut window, gl_config) =
            glutin_winit::DisplayBuilder::new() // let glutin-winit helper crate handle the complex parts of opengl context creation
                .with_preference(glutin_winit::ApiPrefence::FallbackEgl) // https://github.com/emilk/egui/issues/2520#issuecomment-1367841150
                .with_window_builder(Some(winit_window_builder.clone()))
                .build(
                    event_loop,
                    config_template_builder,
                    |mut config_iterator| {
                        config_iterator.next().expect(
                            "failed to find a matching configuration for creating glutin config",
                        )
                    },
                )
                .expect("failed to create gl_config");
        let gl_display = gl_config.display();
        tracing::debug!("found gl_config: {:?}", &gl_config);

        let raw_window_handle = window.as_ref().map(|w| w.raw_window_handle());
        tracing::debug!("raw window handle: {:?}", raw_window_handle);
        let context_attributes =
            glutin::context::ContextAttributesBuilder::new().build(raw_window_handle);
        // by default, glutin will try to create a core opengl context. but, if it is not available, try to create a gl-es context using this fallback attributes
        let fallback_context_attributes = glutin::context::ContextAttributesBuilder::new()
            .with_context_api(glutin::context::ContextApi::Gles(None))
            .build(raw_window_handle);
        let not_current_gl_context = unsafe {
            gl_display
                    .create_context(&gl_config, &context_attributes)
                    .unwrap_or_else(|_| {
                        tracing::debug!("failed to create gl_context with attributes: {:?}. retrying with fallback context attributes: {:?}",
                            &context_attributes,
                            &fallback_context_attributes);
                        gl_config
                            .display()
                            .create_context(&gl_config, &fallback_context_attributes)
                            .expect("failed to create context even with fallback attributes")
                    })
        };

        // this is where the window is created, if it has not been created while searching for suitable gl_config
        let window = window.take().unwrap_or_else(|| {
            tracing::debug!("window doesn't exist yet. creating one now with finalize_window");
            glutin_winit::finalize_window(event_loop, winit_window_builder.clone(), &gl_config)
                .expect("failed to finalize glutin window")
        });
        let (width, height): (u32, u32) = window.inner_size().into();
        let width = std::num::NonZeroU32::new(width.at_least(1)).unwrap();
        let height = std::num::NonZeroU32::new(height.at_least(1)).unwrap();
        let surface_attributes =
            glutin::surface::SurfaceAttributesBuilder::<glutin::surface::WindowSurface>::new()
                .build(window.raw_window_handle(), width, height);
        tracing::debug!(
            "creating surface with attributes: {:?}",
            &surface_attributes
        );
        let gl_surface = unsafe {
            gl_display
                .create_window_surface(&gl_config, &surface_attributes)
                .unwrap()
        };
        tracing::debug!("surface created successfully: {gl_surface:?}.making context current");
        let gl_context = not_current_gl_context.make_current(&gl_surface).unwrap();

        gl_surface
            .set_swap_interval(
                &gl_context,
                glutin::surface::SwapInterval::Wait(std::num::NonZeroU32::new(1).unwrap()),
            )
            .unwrap();

        GlutinWindowContext {
            window,
            gl_context,
            gl_display,
            gl_surface,
        }
    }

    fn window(&self) -> &winit::window::Window {
        &self.window
    }

    fn resize(&self, physical_size: winit::dpi::PhysicalSize<u32>) {
        use glutin::surface::GlSurface;
        self.gl_surface.resize(
            &self.gl_context,
            physical_size.width.try_into().unwrap(),
            physical_size.height.try_into().unwrap(),
        );
    }

    fn swap_buffers(&self) -> glutin::error::Result<()> {
        use glutin::surface::GlSurface;
        self.gl_surface.swap_buffers(&self.gl_context)
    }

    fn get_proc_address(&self, addr: &std::ffi::CStr) -> *const std::ffi::c_void {
        use glutin::display::GlDisplay;
        self.gl_display.get_proc_address(addr)
    }
}

pub(crate) fn main(args: Arguments) {
    let clear_color = [0.1, 0.1, 0.1];

    let event_loop = winit::event_loop::EventLoopBuilder::with_user_event().build();
    let (gl_window, gl) = create_display(&event_loop);
    let gl = std::sync::Arc::new(gl);

    let mut egui_glow = EguiGlow::new(&event_loop, gl.clone(), None);
    egui_glow.run(gl_window.window(), |_| {}, None); // needed for loading fonts

    let mut client = eterm::Client::new(args.url);

    event_loop.run(move |event, _, control_flow| {
        let mut redraw = || {
            let server_frame =
                client.update(&egui_glow.egui_ctx, egui_glow.egui_ctx.pixels_per_point());

            let quit = false;

            let (repaint_after, raw_input) = egui_glow.run(
                gl_window.window(),
                |egui_ctx| {
                    client_gui(egui_ctx, &client);
                },
                server_frame,
            );

            client.send_input(raw_input);

            *control_flow = if quit {
                winit::event_loop::ControlFlow::Exit
            } else if repaint_after.is_zero() {
                gl_window.window().request_redraw();
                winit::event_loop::ControlFlow::Poll
            } else if let Some(repaint_after_instant) =
                std::time::Instant::now().checked_add(repaint_after)
            {
                winit::event_loop::ControlFlow::WaitUntil(repaint_after_instant)
            } else {
                winit::event_loop::ControlFlow::Wait
            };

            {
                unsafe {
                    use glow::HasContext as _;
                    gl.clear_color(clear_color[0], clear_color[1], clear_color[2], 1.0);
                    gl.clear(glow::COLOR_BUFFER_BIT);
                }

                // draw things behind egui here

                egui_glow.paint(gl_window.window());

                // draw things on top of egui here

                gl_window.swap_buffers().unwrap();
                gl_window.window().set_visible(true);
            }
        };

        match event {
            // Platform-dependent event handlers to workaround a winit bug
            // See: https://github.com/rust-windowing/winit/issues/987
            // See: https://github.com/rust-windowing/winit/issues/1619
            winit::event::Event::RedrawEventsCleared if cfg!(windows) => redraw(),
            winit::event::Event::RedrawRequested(_) if !cfg!(windows) => redraw(),

            winit::event::Event::WindowEvent { event, .. } => {
                use winit::event::WindowEvent;
                if matches!(event, WindowEvent::CloseRequested | WindowEvent::Destroyed) {}

                match &event {
                    WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                        *control_flow = winit::event_loop::ControlFlow::Exit;
                    }
                    winit::event::WindowEvent::Resized(physical_size) => {
                        gl_window.resize(*physical_size);
                    }
                    winit::event::WindowEvent::ScaleFactorChanged {
                        new_inner_size,
                        scale_factor,
                    } => {
                        gl_window.resize(**new_inner_size);
                        egui_glow
                            .egui_ctx
                            .set_pixels_per_point(*scale_factor as f32);
                        egui_glow
                            .egui_winit
                            .set_pixels_per_point(*scale_factor as f32)
                    }
                    _ => {}
                }

                let event_response = egui_glow.on_event(&event);

                if event_response.repaint {
                    gl_window.window().request_redraw();
                }
            }
            winit::event::Event::LoopDestroyed => {
                egui_glow.destroy();
            }
            winit::event::Event::NewEvents(winit::event::StartCause::ResumeTimeReached {
                ..
            }) => {
                gl_window.window().request_redraw();
            }

            _ => (),
        }
    });
}

fn create_display(
    event_loop: &winit::event_loop::EventLoopWindowTarget<()>,
) -> (GlutinWindowContext, glow::Context) {
    let glutin_window_context = unsafe { GlutinWindowContext::new(event_loop) };
    let gl = unsafe {
        glow::Context::from_loader_function(|s| {
            let s = std::ffi::CString::new(s)
                .expect("failed to construct C string from string for gl proc address");

            glutin_window_context.get_proc_address(&s)
        })
    };

    (glutin_window_context, gl)
}

/// Use [`egui`] from a [`glow`] app based on [`winit`].
pub struct EguiGlow {
    pub egui_ctx: egui::Context,
    pub egui_winit: egui_winit::State,
    pub painter: Painter,

    shapes: Vec<egui::epaint::ClippedShape>,
    received_shapes: Vec<egui::epaint::ClippedPrimitive>,
    textures_delta: egui::TexturesDelta,
}

impl EguiGlow {
    /// For automatic shader version detection set `shader_version` to `None`.
    pub fn new<E>(
        event_loop: &winit::event_loop::EventLoopWindowTarget<E>,
        gl: std::sync::Arc<glow::Context>,
        shader_version: Option<ShaderVersion>,
    ) -> Self {
        let painter = Painter::new(gl, "", shader_version)
            .map_err(|error| {
                tracing::error!("error occurred in initializing painter:\n{}", error);
            })
            .unwrap();

        Self {
            egui_ctx: Default::default(),
            egui_winit: egui_winit::State::new(event_loop),
            painter,
            shapes: Default::default(),
            received_shapes: Default::default(),
            textures_delta: Default::default(),
        }
    }

    pub fn on_event(&mut self, event: &winit::event::WindowEvent<'_>) -> EventResponse {
        self.egui_winit.on_event(&self.egui_ctx, event)
    }

    /// Returns the `Duration` of the timeout after which egui should be repainted even if there's no new events.
    ///
    /// Call [`Self::paint`] later to paint.
    pub fn run(
        &mut self,
        window: &winit::window::Window,
        run_ui: impl FnMut(&egui::Context),
        server_frame: Option<eterm::EguiFrame>,
    ) -> (std::time::Duration, RawInput) {
        let raw_input = self.egui_winit.take_egui_input(window);

        let egui::FullOutput {
            mut platform_output,
            repaint_after,
            textures_delta,
            shapes,
        } = self.egui_ctx.run(raw_input.clone(), run_ui);

        // The received server egui primitives.
        if let Some(server_frame) = server_frame {
            if !server_frame.clipped_meshes.is_empty() {
                self.received_shapes = server_frame.clipped_meshes;
                platform_output.append(server_frame.platform_output);
            }
        }

        self.egui_winit
            .handle_platform_output(window, &self.egui_ctx, platform_output);
        self.shapes = shapes;
        self.textures_delta.append(textures_delta);
        (repaint_after, raw_input)
    }

    /// Paint the results of the last call to [`Self::run`].
    pub fn paint(&mut self, window: &winit::window::Window) {
        let shapes = std::mem::take(&mut self.shapes);
        let mut textures_delta = std::mem::take(&mut self.textures_delta);

        for (id, image_delta) in textures_delta.set {
            self.painter.set_texture(id, &image_delta);
        }

        let mut clipped_primitives = self.egui_ctx.tessellate(shapes);
        let dimensions: [u32; 2] = window.inner_size().into();

        clipped_primitives.extend_from_slice(self.received_shapes.as_slice());

        self.painter.paint_primitives(
            dimensions,
            self.egui_ctx.pixels_per_point(),
            &clipped_primitives,
        );

        for id in textures_delta.free.drain(..) {
            self.painter.free_texture(id);
        }
    }

    /// Call to release the allocated graphics resources.
    pub fn destroy(&mut self) {
        self.painter.destroy();
    }
}

fn client_gui(ctx: &egui::Context, client: &eterm::Client) {
    // Chose a theme that sets us apart from the server:
    let mut visuals = ctx.style().visuals.clone();
    let panel_background = if visuals.dark_mode {
        egui::Color32::from_rgb(55, 0, 105)
    } else {
        egui::Color32::from_rgb(255, 240, 0)
    };
    visuals.widgets.noninteractive.bg_fill = panel_background;
    ctx.set_visuals(visuals);

    egui::Window::new("Eterm Client Stats")
        .default_pos(Pos2::new(300.0, 200.0))
        .show(ctx, |ui| {
            ui.horizontal(|ui| {
                client_info_bar(ui, client);
            });
        });
}

fn client_info_bar(ui: &mut egui::Ui, client: &eterm::Client) {
    if client.is_connected() {
        ui.vertical(|ui| {
            ui.label(format!("Connected to {}", client.addr(),));
            ui.separator();
            ui.label(format!(
                "{:.2} MB/s download",
                client.bytes_per_second() * 1e-6
            ));
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
        });
    } else {
        ui.label(format!("Connecting to {}…", client.addr()));
    }
}
