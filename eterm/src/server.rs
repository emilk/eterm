use std::{
    collections::HashMap,
    net::{SocketAddr, TcpListener},
};

use anyhow::Context as _;
use egui::RawInput;

use crate::{net_shape::ClippedNetShape, ClientToServerMessage};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClientId(u64);

pub struct Server {
    next_client_id: u64,
    tcp_listener: TcpListener,
    clients: HashMap<SocketAddr, Client>,
    minimum_update_interval: f32,
}

impl Server {
    /// Start listening for connections on this addr (e.g. "0.0.0.0:8585")
    ///
    /// # Errors
    /// Can fail if the port is already taken.
    pub fn new(bind_addr: &str) -> anyhow::Result<Self> {
        let tcp_listener = TcpListener::bind(bind_addr).context("binding server TCP socket")?;
        tcp_listener
            .set_nonblocking(true)
            .context("TCP set_nonblocking")?;

        Ok(Self {
            next_client_id: 0,
            tcp_listener,
            clients: Default::default(),
            minimum_update_interval: 1.0,
        })
    }

    /// Send a new frame to each client at least this often.
    /// Default: one second.
    pub fn set_minimum_update_interval(&mut self, seconds: f32) {
        self.minimum_update_interval = seconds;
    }

    /// Call frequently (e.g. 60 times per second) with the ui you'd like to show to clients.
    ///
    /// # Errors
    /// Underlying TCP errors.
    pub fn show(&mut self, mut do_ui: impl FnMut(&egui::CtxRef, ClientId)) -> anyhow::Result<()> {
        self.show_dyn(&mut do_ui)
    }

    fn show_dyn(&mut self, do_ui: &mut dyn FnMut(&egui::CtxRef, ClientId)) -> anyhow::Result<()> {
        self.accept_new_clients()?;
        self.try_receive();

        for client in self.clients.values_mut() {
            client.show(do_ui, self.minimum_update_interval);
        }
        Ok(())
    }

    /// non-blocking
    fn accept_new_clients(&mut self) -> anyhow::Result<()> {
        loop {
            match self.tcp_listener.accept() {
                Ok((tcp_stream, client_addr)) => {
                    tcp_stream
                        .set_nonblocking(true)
                        .context("stream.set_nonblocking")?;
                    let tcp_endpoint = crate::TcpEndpoint { tcp_stream };

                    // reuse existing client - especially the egui context
                    // which contains things like window positons:
                    let clients = &mut self.clients;
                    let next_client_id = &mut self.next_client_id;
                    let client = clients.entry(client_addr).or_insert_with(|| {
                        let client_id = ClientId(*next_client_id);
                        *next_client_id += 1;

                        Client {
                            client_id,
                            addr: client_addr,
                            tcp_endpoint: None,
                            start_time: std::time::Instant::now(),
                            frame_index: 0,
                            egui_ctx: Default::default(),
                            input: None,
                            client_time: None,
                            last_update: None,
                            last_visuals: Default::default(),
                        }
                    });

                    client.tcp_endpoint = Some(tcp_endpoint);

                    // TODO: send egui::FontDefinitions to client

                    tracing::info!("{} connected", client.info());
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    break; // No (more) new clients
                }
                Err(err) => {
                    anyhow::bail!("eterm server TCP error: {:?}", err);
                }
            }
        }
        Ok(())
    }

    /// non-blocking
    fn try_receive(&mut self) {
        for client in self.clients.values_mut() {
            client.try_receive();
        }
    }
}

// ----------------------------------------------------------------------------

struct Client {
    client_id: ClientId,
    addr: SocketAddr,
    tcp_endpoint: Option<crate::TcpEndpoint>,
    start_time: std::time::Instant,
    frame_index: u64,
    egui_ctx: egui::CtxRef,
    /// Set when there is something to do. Cleared after painting.
    input: Option<egui::RawInput>,
    /// The client time of the last input we got from them.
    client_time: Option<f64>,
    last_update: Option<std::time::Instant>,
    last_visuals: Vec<ClippedNetShape>,
}

impl Client {
    fn disconnect(&mut self) {
        self.tcp_endpoint = None;
        self.last_visuals = Default::default();
    }

    fn show(
        &mut self,
        do_ui: &mut dyn FnMut(&egui::CtxRef, ClientId),
        minimum_update_interval: f32,
    ) {
        if self.tcp_endpoint.is_none() {
            return;
        }

        let client_time = self.client_time.take();

        let mut input = match self.input.take() {
            Some(input) => input,
            None => {
                let time_since_last_update =
                    self.last_update.map_or(f32::INFINITY, |last_update| {
                        last_update.elapsed().as_secs_f32()
                    });
                if time_since_last_update > minimum_update_interval {
                    Default::default()
                } else {
                    return;
                }
            }
        };

        self.last_update = Some(std::time::Instant::now());

        // Ignore client time:
        input.time = Some(self.start_time.elapsed().as_secs_f64());

        let (mut output, clipped_shapes) = self
            .egui_ctx
            .run(input, |egui_ctx| do_ui(egui_ctx, self.client_id));

        let clipped_net_shapes = crate::net_shape::to_clipped_net_shapes(clipped_shapes);

        let needs_repaint = output.needs_repaint;
        output.needs_repaint = false; // so we can compare below

        if output == Default::default() && clipped_net_shapes == self.last_visuals {
            // No change - save bandwidth and send nothing
        } else {
            let frame_index = self.frame_index;
            self.frame_index += 1;

            let message = crate::ServerToClientMessage::Frame {
                frame_index,
                output,
                clipped_net_shapes: clipped_net_shapes.clone(),
                client_time,
            };

            self.last_visuals = clipped_net_shapes;
            self.send_message(&message);
        }

        if needs_repaint {
            // eprintln!("frame {} painted, needs_repaint", frame_index);
            // Reschedule asap (don't wait for client) to request it.
            self.input = Some(Default::default());
        } else {
            // eprintln!("frame {} painted", frame_index);
        }
    }

    fn info(&self) -> String {
        format!("Client {} ({})", self.client_id.0, self.addr)
    }

    fn send_message(&mut self, message: &impl serde::Serialize) {
        if let Some(tcp_endpoint) = &mut self.tcp_endpoint {
            match tcp_endpoint.send_message(&message) {
                Ok(()) => {}
                Err(err) => {
                    tracing::error!(
                        "Failed to send to client {:?} {}: {:?}. Disconnecting.",
                        self.client_id,
                        self.addr,
                        crate::error_display_chain(err.as_ref())
                    );
                    self.disconnect();
                }
            }
        }
    }

    /// non-blocking
    fn try_receive(&mut self) {
        loop {
            let tcp_endpoint = match &mut self.tcp_endpoint {
                Some(tcp_endpoint) => tcp_endpoint,
                None => return,
            };

            let message = match tcp_endpoint.try_receive_message() {
                Ok(None) => {
                    return;
                }
                Ok(Some(message)) => message,
                Err(err) => {
                    tracing::error!(
                        "Failed to read from client {}: {:?}. Disconnecting.",
                        self.info(),
                        crate::error_display_chain(err.as_ref())
                    );
                    self.disconnect();
                    return;
                }
            };

            match message {
                ClientToServerMessage::Input {
                    raw_input,
                    client_time,
                } => {
                    // eprintln!("Received new input");
                    self.input(raw_input);
                    self.client_time = Some(client_time);
                    // keep polling for more messages
                }
                ClientToServerMessage::Goodbye => {
                    self.disconnect();
                    return;
                }
            }
        }
    }

    fn input(&mut self, new_input: RawInput) {
        match &mut self.input {
            None => {
                self.input = Some(new_input);
            }
            Some(existing_input) => {
                existing_input.append(new_input);
            }
        }
    }
}
