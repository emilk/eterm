use std::sync::{
    atomic::{AtomicBool, Ordering::SeqCst},
    mpsc::{self},
    Arc,
};

use egui::{util::History, RawInput};
use parking_lot::Mutex;

use crate::{ClientToServerMessage, EguiFrame, ServerToClientMessage, TcpEndpoint};

pub struct Client {
    addr: String,
    connected: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    outgoing_msg_tx: mpsc::Sender<ClientToServerMessage>,
    incoming_msg_rx: mpsc::Receiver<ServerToClientMessage>,

    latest_frame: Option<EguiFrame>,

    bandwidth_history: Arc<Mutex<History<f32>>>,
    frame_size_history: Arc<Mutex<History<f32>>>,
    latency_history: History<f32>,
    frame_history: History<()>,
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
        let mut bandwidth_history = Arc::new(Mutex::new(History::new(0..200, 2.0)));
        let mut frame_size_history = Arc::new(Mutex::new(History::new(1..100, 0.5)));

        let (outgoing_msg_tx, mut outgoing_msg_rx) = mpsc::channel();
        let (mut incoming_msg_tx, incoming_msg_rx) = mpsc::channel();

        let client = Self {
            addr: addr.clone(),
            connected: connected.clone(),
            alive: alive.clone(),
            outgoing_msg_tx,
            incoming_msg_rx,
            latest_frame: Default::default(),
            bandwidth_history: bandwidth_history.clone(),
            frame_size_history: frame_size_history.clone(),
            latency_history: History::new(1..100, 1.0),
            frame_history: History::new(2..100, 1.0),
        };

        std::thread::spawn(move || {
            tracing::info!("Connecting to {}…", addr);
            while alive.load(SeqCst) {
                match std::net::TcpStream::connect(&addr) {
                    Ok(tcp_stream) => {
                        tracing::info!("Connected!");
                        connected.store(true, SeqCst);
                        if let Err(err) = run(
                            tcp_stream,
                            &mut outgoing_msg_rx,
                            &mut incoming_msg_tx,
                            &mut bandwidth_history,
                            &mut frame_size_history,
                        ) {
                            tracing::info!(
                                "Connection lost: {}",
                                crate::error_display_chain(err.as_ref())
                            );
                        } else {
                            tracing::info!("Connection closed.",);
                        }
                        connected.store(false, SeqCst);
                    }
                    Err(err) => {
                        tracing::debug!("Failed to connect to {}: {}", addr, err);
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

    pub fn send_input(&self, raw_input: RawInput) {
        self.outgoing_msg_tx
            .send(ClientToServerMessage::Input {
                raw_input,
                client_time: now(),
            })
            .ok();
    }

    /// Estimated bandwidth use (downstream).
    pub fn bytes_per_second(&self) -> f32 {
        self.bandwidth_history.lock().bandwidth().unwrap_or(0.0)
    }

    /// Estimated size of one frame packet
    pub fn average_frame_packet_size(&self) -> Option<f32> {
        self.frame_size_history.lock().average()
    }

    /// Smoothed round-trip-time estimate in seconds.
    pub fn latency(&self) -> Option<f32> {
        self.latency_history.average()
    }

    /// Smoothed estimate of the adaptive frames per second.
    pub fn adaptive_fps(&self) -> Option<f32> {
        self.frame_history.rate()
    }

    /// Retrieved new events, and gives back what to do.
    ///
    /// Return `None` when there is nothing new.
    pub fn update(&mut self, egui_ctx: &egui::Context, pixels_per_point: f32) -> Option<EguiFrame> {
        egui_ctx.fonts(|f| f.begin_frame(pixels_per_point, f.max_texture_side()));

        while let Ok(msg) = self.incoming_msg_rx.try_recv() {
            match msg {
                ServerToClientMessage::Fonts { font_definitions } => {
                    egui_ctx.set_fonts(font_definitions.clone());
                }
                ServerToClientMessage::Frame {
                    frame_index,
                    platform_output,
                    clipped_net_shapes,
                    client_time,
                } => {
                    let clipped_shapes = egui_ctx.fonts(|fonts| {
                        crate::net_shape::from_clipped_net_shapes(fonts, clipped_net_shapes)
                    });

                    let clipped_primitives = egui_ctx.tessellate(clipped_shapes);

                    let latest_frame = self.latest_frame.get_or_insert_with(EguiFrame::default);
                    latest_frame.frame_index = frame_index;
                    latest_frame.platform_output.append(platform_output);
                    latest_frame.clipped_meshes = clipped_primitives;

                    if let Some(client_time) = client_time {
                        let rtt = (now() - client_time) as f32;
                        self.latency_history.add(now(), rtt);
                    }

                    self.frame_history.add(now(), ());
                }
            }
        }

        self.bandwidth_history.lock().flush(now());
        self.frame_size_history.lock().flush(now());

        self.latency_history.flush(now());
        self.frame_history.flush(now());

        self.latest_frame.take()
    }
}

fn run(
    tcp_stream: std::net::TcpStream,
    outgoing_msg_rx: &mut mpsc::Receiver<ClientToServerMessage>,
    incoming_msg_tx: &mut mpsc::Sender<ServerToClientMessage>,
    bandwidth_history: &mut Arc<Mutex<History<f32>>>,
    frame_size_history: &mut Arc<Mutex<History<f32>>>,
) -> anyhow::Result<()> {
    use anyhow::Context as _;

    tcp_stream
        .set_nonblocking(true)
        .context("TCP set_nonblocking")?;

    let mut tcp_endpoint = TcpEndpoint { tcp_stream };

    loop {
        loop {
            match outgoing_msg_rx.try_recv() {
                Ok(message) => {
                    tcp_endpoint.send_message(&message)?;
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
            if let ServerToClientMessage::Frame { .. } = &message {
                frame_size_history.lock().add(now(), packet.len() as f32);
            }
            incoming_msg_tx.send(message)?;
        }

        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

fn now() -> f64 {
    std::time::UNIX_EPOCH.elapsed().unwrap().as_secs_f64()
}
