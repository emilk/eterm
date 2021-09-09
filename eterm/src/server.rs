use std::{
    collections::HashMap,
    net::{SocketAddr, TcpListener},
    sync::Arc,
};

use anyhow::Context as _;
use egui::RawInput;
use parking_lot::Mutex;

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
        self.server_impl.accept_new_clients()?;
        self.server_impl.try_receive();

        self.server_impl.clients.retain(|_, client| {
            let mut client_state = client.state.lock();
            if let Some(input) = client_state.input.take() {
                let frame_index = client_state.frame_index;
                client_state.frame_index += 1;
                client_state.egui_ctx.begin_frame(input);
                show(&client_state.egui_ctx, client.client_id);
                let (output, clipped_shapes) = client_state.egui_ctx.end_frame();
                drop(client_state);

                let needs_repaint = output.needs_repaint;

                let clipped_net_shapes = crate::net_shape::to_clipped_net_shapes(clipped_shapes);
                let message = crate::ServerToClientMessage::Frame {
                    frame_index,
                    output,
                    clipped_net_shapes,
                };

                match client.tcp_endpoint.send_message(&message) {
                    Ok(()) => {}
                    Err(err) => {
                        log::error!(
                            "Failed to send to client {:?} {}: {:?}. Disconnecting.",
                            client.client_id,
                            client.addr,
                            crate::error_display_chain(err.as_ref())
                        );
                        return false;
                    }
                }

                if needs_repaint {
                    // eprintln!("frame {} painted, needs_repaint", frame_index);
                    // Reschedule asap (don't wait for client) to request it.
                    client.state.lock().input = Some(Default::default());
                } else {
                    // eprintln!("frame {} painted", frame_index);
                }
            }
            true
        });
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
    clients: HashMap<ClientId, TcpClient>,
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

                    let client_id = ClientId(self.next_client_id);
                    self.next_client_id += 1;

                    let client = TcpClient {
                        client_id,
                        addr: client_addr,
                        tcp_endpoint,
                        state: Default::default(),
                    };

                    // TODO: send egui::FontDefinitions to client

                    log::info!("{} connected (Client id = {})", client.addr, client_id.0);
                    self.clients.insert(client_id, client);
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
        self.clients.retain(|_, client| {
            loop {
                let message = match client.tcp_endpoint.try_receive_message() {
                    Ok(None) => {
                        return true;
                    }
                    Ok(Some(message)) => message,
                    Err(err) => {
                        log::error!(
                            "Failed to read from client {:?} {}: {:?}. Disconnecting.",
                            client.client_id,
                            client.addr,
                            err
                        );
                        return false;
                    }
                };

                match message {
                    ClientToServerMessage::Input { raw_input } => {
                        // eprintln!("Received new input");
                        client.state.lock().input(raw_input);
                        // keep polling for more messages
                    }
                    ClientToServerMessage::Goodbye => {
                        return false;
                    }
                }
            }
        });
    }
}

// ----------------------------------------------------------------------------

pub struct TcpClient {
    client_id: ClientId,
    addr: SocketAddr,
    tcp_endpoint: crate::TcpEndpoint,
    state: Arc<Mutex<ClientState>>,
}

#[derive(Default)]
pub struct ClientState {
    frame_index: u64,
    egui_ctx: egui::CtxRef,
    input: Option<egui::RawInput>,
}

impl ClientState {
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
