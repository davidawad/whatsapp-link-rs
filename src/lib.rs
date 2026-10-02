//! Link a WhatsApp account and read and send messages from Rust.
//!
//! The crate spawns [`wuzapi`](https://github.com/asternic/wuzapi) in its
//! stdio mode (`wuzapi -mode=stdio -datadir=<dir>`) and speaks its
//! newline-delimited JSON-RPC over the child's stdin and stdout.
//!
//! ```no_run
//! use whatsapp_link::{Config, Recipient, Wuzapi};
//!
//! # fn main() -> whatsapp_link::Result<()> {
//! let wa = Wuzapi::spawn(Config::from_env())?;
//! wa.connect()?;
//! let to: Recipient = "+15550100".parse()?;
//! let sent = wa.send_text(&to, "hello")?;
//! println!("sent as {}", sent.id);
//! # Ok(())
//! # }
//! ```
//!
//! Automating WhatsApp through an unofficial client may violate WhatsApp's
//! terms of service and can get an account restricted.
//!
//! The `native` cargo feature is a placeholder for a future pure-Rust backend
//! (for example the `whatsapp-rust` crate) and currently changes nothing.

mod client;
mod config;
mod error;
mod event;
mod guard;
mod model;
mod qr;
mod rpc;

pub use client::{LinkEvent, PairCode, QrStream, Wuzapi};
pub use config::{derive_token, env, Config};
pub use error::{Error, Result};
pub use event::{Event, Events};
pub use guard::DataDirGuard;
pub use model::{
    Chat, History, Item, Jid, LinkState, MediaKind, Message, MessageId, Reaction, Recipient,
    SentMessage, Status,
};
pub use qr::{render_terminal, QrCode};
