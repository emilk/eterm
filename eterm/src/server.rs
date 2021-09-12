use std::{
    collections::HashMap,
    net::{SocketAddr, TcpListener},
};

use anyhow::Context as _;
use egui::RawInput;

use crate::ClientToServerMessage;

pub struct Server {
    server_impl: ServerImpl,
}

impl Server {
    /// Start listening for connections on this addr (e.g. "0.0.0.0:8585")
    pub fn new(bind_addr: &str) -> anyhow::Result<Self> {
        let tcp_listener = TcpListener::bind(bind_addr).context("binding server TCP socket")?;
        tcp_listener
            .set_nonblocking(true)
            .context("TCP set_nonblocking")?;

        let server_impl = ServerImpl {
            next_client_id: 0,
            tcp_listener,
            clients: Default::default(),
        };

        Ok(Self { server_impl })
    }

    pub fn show(&mut self, mut show: impl FnMut(&egui::CtxRef, ClientId)) -> anyhow::Result<()> {
        self.show_dyn(&mut show)
    }

    fn show_dyn(&mut self, show: &mut dyn FnMut(&egui::CtxRef, ClientId)) -> anyhow::Result<()> {
        self.server_impl.accept_new_clients()?;
        self.server_impl.try_receive();

        for client in self.server_impl.clients.values_mut() {
            client.show(show);
        }
        Ok(())
    }
}

// ----------------------------------------------------------------------------

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ClientId(u64);

// ----------------------------------------------------------------------------

struct ServerImpl {
    next_client_id: u64,
    tcp_listener: TcpListener,
    clients: HashMap<SocketAddr, Client>,
}

impl ServerImpl {
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
                        }
                    });

                    client.tcp_endpoint = Some(tcp_endpoint);

                    // TODO: send egui::FontDefinitions to client

                    log::info!("{} connected", client.info());
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
}

impl Client {
    fn show(&mut self, show: &mut dyn FnMut(&egui::CtxRef, ClientId)) {
        if self.tcp_endpoint.is_none() {
            return;
        }

        let mut input = match self.input.take() {
            Some(input) => input,
            None => {
                return; // TODO: updarte peroiodically
            }
        };

        // Ignore client time:
        input.time = Some(self.start_time.elapsed().as_secs_f64());

        let frame_index = self.frame_index;
        self.frame_index += 1;
        self.egui_ctx.begin_frame(input);
        show(&self.egui_ctx, self.client_id);
        let (output, clipped_shapes) = self.egui_ctx.end_frame();

        let needs_repaint = output.needs_repaint;

        let clipped_net_shapes = crate::net_shape::to_clipped_net_shapes(clipped_shapes);
        let message = crate::ServerToClientMessage::Frame {
            frame_index,
            output,
            clipped_net_shapes,
        };

        self.send_message(&message);

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
                    log::error!(
                        "Failed to send to client {:?} {}: {:?}. Disconnecting.",
                        self.client_id,
                        self.addr,
                        crate::error_display_chain(err.as_ref())
                    );
                    self.tcp_endpoint = None;
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
                    log::error!(
                        "Failed to read from client {}: {:?}. Disconnecting.",
                        self.info(),
                        crate::error_display_chain(err.as_ref())
                    );
                    self.tcp_endpoint = None;
                    return;
                }
            };

            match message {
                ClientToServerMessage::Input { raw_input } => {
                    // eprintln!("Received new input");
                    self.input(raw_input);
                    // keep polling for more messages
                }
                ClientToServerMessage::Goodbye => {
                    self.tcp_endpoint = None;
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
