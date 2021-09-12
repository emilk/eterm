use std::sync::{
    atomic::{AtomicBool, Ordering::SeqCst},
    mpsc::{self},
    Arc,
};

use egui::{mutex::Mutex, text::Fonts, util::History, RawInput};

use crate::{ClientToServerMessage, EguiFrame, ServerToClientMessage, TcpEndpoint};

pub struct Client {
    addr: String,
    connected: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    input_tx: mpsc::Sender<egui::RawInput>,
    server_msg_rx: mpsc::Receiver<ServerToClientMessage>,

    font_definitions: egui::FontDefinitions,
    fonts: Option<Fonts>,
    latest_frame: Option<EguiFrame>,

    bandwidth_history: Arc<Mutex<History<f32>>>,
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
        let mut bandwidth_history = Arc::new(Mutex::new(History::from_max_len_age(200, 2.0)));

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
            bandwidth_history: bandwidth_history.clone(),
        };

        std::thread::spawn(move || {
            log::info!("Connecting to {}…", addr);
            while alive.load(SeqCst) {
                match std::net::TcpStream::connect(&addr) {
                    Ok(tcp_stream) => {
                        log::info!("Connected!",);
                        connected.store(true, SeqCst);
                        if let Err(err) = run(
                            tcp_stream,
                            &mut input_rx,
                            &mut server_msg_tx,
                            &mut bandwidth_history,
                        ) {
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
    pub fn is_connected(&self) -> bool {
        self.connected.load(SeqCst)
    }

    pub fn send_input(&self, input: RawInput) {
        self.input_tx.send(input).ok();
    }

    /// Estimated bandwidth use (down)
    pub fn bytes_per_second(&self) -> f32 {
        self.bandwidth_history.lock().sum_over_time().unwrap_or(0.0)
    }

    /// Retrieved new events, and gives back what to do.
    ///
    /// Return `None` when there is nothing new.
    pub fn update(&mut self, pixels_per_point: f32) -> Option<EguiFrame> {
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
                        crate::net_shape::from_clipped_net_shapes(fonts, clipped_net_shapes);
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

                    let latest_frame = self.latest_frame.get_or_insert_with(EguiFrame::default);
                    latest_frame.frame_index = frame_index;
                    latest_frame.output.append(output);
                    latest_frame.clipped_meshes = clipped_meshes;
                }
            }
        }

        self.latest_frame.take()
    }

    pub fn texture(&self) -> Arc<egui::Texture> {
        self.fonts.as_ref().expect("Call update() first").texture()
    }
}

fn run(
    tcp_stream: std::net::TcpStream,
    input_rx: &mut mpsc::Receiver<egui::RawInput>,
    server_msg_tx: &mut mpsc::Sender<ServerToClientMessage>,
    bandwidth_history: &mut Arc<Mutex<History<f32>>>,
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

        while let Some(packet) = tcp_endpoint.try_receive_packet().context("receive")? {
            bandwidth_history.lock().add(now(), packet.len() as f32);
            let message = crate::decode_message(&packet).context("decode")?;
            server_msg_tx.send(message)?;
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn now() -> f64 {
    std::time::UNIX_EPOCH.elapsed().unwrap().as_secs_f64()
}