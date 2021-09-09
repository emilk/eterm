use std::sync::{
    atomic::{AtomicBool, Ordering::SeqCst},
    mpsc::{self},
    Arc,
};

use egui::{text::Fonts, RawInput};

use crate::{ClientToServerMessage, EguiFrame, ServerToClientMessage, TcpEndpoint};

pub struct Client {
    addr: String,
    connected: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    input_tx: mpsc::Sender<egui::RawInput>,
    server_msg_rx: mpsc::Receiver<ServerToClientMessage>,

    font_definitions: egui::FontDefinitions,
    fonts: Option<Fonts>,
    latest_frame: EguiFrame,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.alive.store(false, SeqCst);
    }
}

impl Client {
    /// Connects to the given eterm server.
    ///
    /// ``` no_run
    /// eterm::Client::new("127.0.0.1:8580".to_owned());
    /// ```
    pub fn new(addr: String) -> Self {
        let alive = Arc::new(AtomicBool::new(true));
        let connected = Arc::new(AtomicBool::new(false));

        let (input_tx, mut input_rx) = mpsc::channel();
        let (mut server_msg_tx, server_msg_rx) = mpsc::channel();

        let client = Self {
            addr: addr.clone(),
            connected: connected.clone(),
            alive: alive.clone(),
            input_tx,
            server_msg_rx,
            font_definitions: Default::default(),
            fonts: None,
            latest_frame: Default::default(),
        };

        std::thread::spawn(move || {
            log::info!("Connecting to {}…", addr);
            while alive.load(SeqCst) {
                match std::net::TcpStream::connect(&addr) {
                    Ok(tcp_stream) => {
                        log::info!("Connected!",);
                        connected.store(true, SeqCst);
                        if let Err(err) = run(tcp_stream, &mut input_rx, &mut server_msg_tx) {
                            log::info!(
                                "Connection lost: {}",
                                crate::error_display_chain(err.as_ref())
                            );
                        } else {
                            log::info!("Connection closed.",);
                        }
                        connected.store(false, SeqCst);
                    }
                    Err(err) => {
                        log::debug!("Failed to connect to {}: {}", addr, err);
                        std::thread::sleep(std::time::Duration::from_secs(1));
                    }
                }
            }
        });

        client
    }

    /// The address we are connected to or trying to connect to.
    pub fn addr(&self) -> &str {
        &self.addr
    }

    /// Are we currently connect to the server?
    pub fn connected(&self) -> bool {
        self.connected.load(SeqCst)
    }

    pub fn send_input(&self, input: RawInput) {
        self.input_tx.send(input).ok();
    }

    /// Retrieved new events, and gives back what to do
    pub fn update(&mut self, pixels_per_point: f32) -> EguiFrame {
        if self.fonts.is_none() {
            self.fonts = Some(Fonts::new(pixels_per_point, self.font_definitions.clone()));
        }
        let fonts = self.fonts.as_mut().unwrap();
        if pixels_per_point != fonts.pixels_per_point() {
            *fonts = Fonts::new(pixels_per_point, self.font_definitions.clone());
        }

        while let Ok(msg) = self.server_msg_rx.try_recv() {
            match msg {
                ServerToClientMessage::Fonts { font_definitions } => {
                    self.font_definitions = font_definitions;
                    *fonts = Fonts::new(pixels_per_point, self.font_definitions.clone());
                }
                ServerToClientMessage::Frame {
                    frame_index,
                    output,
                    clipped_net_shapes,
                } => {
                    let clipped_shapes =
                        crate::net_shape::from_clipped_net_shapes(&fonts, clipped_net_shapes);
                    let tesselator_options =
                        egui::epaint::tessellator::TessellationOptions::from_pixels_per_point(
                            pixels_per_point,
                        );
                    let tex_size = fonts.texture().size();
                    let clipped_meshes = egui::epaint::tessellator::tessellate_shapes(
                        clipped_shapes,
                        tesselator_options,
                        tex_size,
                    );

                    self.latest_frame.frame_index = frame_index;
                    self.latest_frame.output.append(output);
                    self.latest_frame.clipped_meshes = clipped_meshes;
                }
            }
        }

        let output = self.latest_frame.output.take();
        self.latest_frame.output.needs_repaint = output.needs_repaint;

        EguiFrame {
            frame_index: self.latest_frame.frame_index,
            output,
            clipped_meshes: self.latest_frame.clipped_meshes.clone(),
        }
    }

    pub fn texture(&self) -> Arc<egui::Texture> {
        self.fonts.as_ref().expect("Call update() first").texture()
    }
}

fn run(
    tcp_stream: std::net::TcpStream,
    input_rx: &mut mpsc::Receiver<egui::RawInput>,
    server_msg_tx: &mut mpsc::Sender<ServerToClientMessage>,
) -> anyhow::Result<()> {
    use anyhow::Context as _;

    tcp_stream
        .set_nonblocking(true)
        .context("TCP set_nonblocking")?;

    let mut tcp_endpoint = TcpEndpoint { tcp_stream };

    loop {
        loop {
            match input_rx.try_recv() {
                Ok(raw_input) => {
                    tcp_endpoint.send_message(&ClientToServerMessage::Input { raw_input })?;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Ok(());
                }
            }
        }

        while let Some(message) = tcp_endpoint.try_receive_message()? {
            server_msg_tx.send(message)?;
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}
