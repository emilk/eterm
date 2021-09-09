//! * Client: the think client that has a screen, a keyboard etc.
//! * Server: what runs the egui code.

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

mod client;
pub mod net_shape;
mod server;

pub use client::Client;
pub use server::{ClientId, Server};

use std::sync::Arc;

pub type Packet = Arc<[u8]>;

#[derive(Default)]
pub struct EguiFrame {
    pub frame_index: u64,
    pub output: egui::Output,
    pub clipped_meshes: Vec<egui::ClippedMesh>,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum ClientToServerMessage {
    Input { raw_input: egui::RawInput },
    Goodbye,
}

#[derive(serde::Serialize, serde::Deserialize)]
pub enum ServerToClientMessage {
    /// Sent first to all clients so they know how to paint
    /// the [`eterm::NetShape`]:s.
    Fonts {
        font_definitions: egui::FontDefinitions,
    },

    /// What to paint to screen.
    Frame {
        frame_index: u64,
        output: egui::Output,
        clipped_net_shapes: Vec<net_shape::ClippedNetShape>,
    },
}

pub fn encode_message<M: ?Sized + serde::Serialize>(message: &M) -> anyhow::Result<Packet> {
    use anyhow::Context as _;
    use bincode::Options as _;

    let bincoded = bincode::options().serialize(message).context("bincode")?;

    const ZSTD_LEVEL: i32 = 5;
    let compressed =
        zstd::encode_all(std::io::Cursor::new(&bincoded), ZSTD_LEVEL).context("zstd")?;

    Ok(compressed.into())
}

pub fn decode_message<M: serde::de::DeserializeOwned>(packet: &[u8]) -> anyhow::Result<M> {
    use anyhow::Context as _;
    use bincode::Options as _;

    let bincoded = zstd::decode_all(packet).context("zstd")?;

    let message = bincode::options()
        .deserialize(&bincoded)
        .context("bincode")?;

    Ok(message)
}

/// Show full cause chain in a single line
pub(crate) fn error_display_chain(error: &dyn std::error::Error) -> String {
    let mut s = error.to_string();
    if let Some(source) = error.source() {
        s.push_str(" -> ");
        s.push_str(&error_display_chain(source));
    }
    s
}

// ----------------------------------------------------------------------------

pub struct TcpEndpoint {
    tcp_stream: std::net::TcpStream,
}

impl TcpEndpoint {
    /// returns immediately if there is nothing to read
    fn try_receive_packet(&mut self) -> anyhow::Result<Option<Packet>> {
        use std::io::Read as _;

        // All messages are length-prefixed by u32 (LE).
        let mut length = [0_u8; 4];
        match self.tcp_stream.peek(&mut length) {
            Ok(4) => {}
            Ok(_) => {
                return Ok(None);
            }
            Err(err) => {
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(None);
                } else {
                    return Err(err.into());
                }
            }
        }

        let length = u32::from_le_bytes(length) as usize;

        if length > 32_000_000 {
            anyhow::bail!("Refusing packet of {:.1} MB", length as f32 * 1e-6);
        }

        // See if we have the whole packet yet:
        let mut length_and_packet = vec![0_u8; 4 + length];
        match self.tcp_stream.peek(&mut length_and_packet) {
            Ok(bytes_read) => {
                if bytes_read != length_and_packet.len() {
                    return Ok(None); // not yet!
                }
            }
            Err(err) => {
                if err.kind() == std::io::ErrorKind::WouldBlock {
                    return Ok(None);
                } else {
                    return Err(err.into());
                }
            }
        }

        // consume the bytes:
        self.tcp_stream.read_exact(&mut length_and_packet)?;

        let packet = &length_and_packet[4..];

        Ok(Some(packet.into()))
    }

    /// returns immediately if there is nothing to read
    fn try_receive_message<M: serde::de::DeserializeOwned>(&mut self) -> anyhow::Result<Option<M>> {
        use anyhow::Context as _;
        match self.try_receive_packet().context("receive")? {
            Some(packet) => {
                let message = crate::decode_message(&packet).context("decode")?;
                Ok(Some(message))
            }
            None => Ok(None),
        }
    }

    fn send_packet(&mut self, packet: &[u8]) -> anyhow::Result<()> {
        let length = packet.len() as u32;
        let length = length.to_le_bytes();
        self.write_all_with_retry(&length)?;
        self.write_all_with_retry(packet)?;
        Ok(())
    }

    fn write_all_with_retry(&mut self, chunk: &[u8]) -> anyhow::Result<()> {
        use std::io::Write as _;
        loop {
            match self.tcp_stream.write_all(chunk) {
                Ok(()) => {
                    return Ok(());
                }
                Err(err) => {
                    if err.kind() == std::io::ErrorKind::WouldBlock {
                        eprintln!("Write would block, sleeping…");
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    } else {
                        anyhow::bail!("{:?}", err);
                    }
                }
            }
        }
    }

    fn send_message<M: serde::Serialize>(&mut self, message: &M) -> anyhow::Result<()> {
        self.send_packet(&encode_message(message)?)
    }
}
