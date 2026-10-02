//! `whatsapp-link`: command-line front end for the `whatsapp_link` library.
//! Output is JSON (one value per line) except for the terminal QR code.

use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use clap::{Args, Parser, Subcommand};
use serde::Serialize;
use serde_json::{json, Value};
use whatsapp_link::{env, Config, Error, Event, Jid, LinkEvent, Recipient, Result, Wuzapi};

#[derive(Parser)]
#[command(
    name = "whatsapp-link",
    version,
    about = "Link a WhatsApp account and read/send messages via wuzapi"
)]
struct Cli {
    #[command(flatten)]
    session: SessionArgs,
    #[command(subcommand)]
    command: Command,
}

/// Every flag also has an environment variable; flags win.
#[derive(Args)]
struct SessionArgs {
    /// Path to the wuzapi binary [default: `wuzapi` from PATH]
    #[arg(long, global = true, env = env::WUZAPI_BIN)]
    wuzapi_bin: Option<PathBuf>,
    /// wuzapi data directory [default: <XDG data dir>/whatsapp-link]
    #[arg(long, global = true, env = env::DATA_DIR)]
    data_dir: Option<PathBuf>,
    /// wuzapi user token [default: derived from the login name]
    #[arg(long, global = true, env = env::USER_TOKEN, hide_env_values = true)]
    user_token: Option<String>,
    /// wuzapi admin token [default: derived from the login name]
    #[arg(long, global = true, env = env::ADMIN_TOKEN, hide_env_values = true)]
    admin_token: Option<String>,
    /// IANA timezone for the child process [default: $TZ or UTC]
    #[arg(long, global = true, env = env::TIMEZONE)]
    timezone: Option<String>,
}

impl SessionArgs {
    fn config(self) -> Config {
        let base = Config::from_env();
        Config {
            wuzapi_bin: self.wuzapi_bin.unwrap_or(base.wuzapi_bin),
            data_dir: self.data_dir.unwrap_or(base.data_dir),
            user_token: self.user_token.unwrap_or(base.user_token),
            admin_token: self.admin_token.unwrap_or(base.admin_token),
            timezone: self.timezone.unwrap_or(base.timezone),
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Print the link state of the session as JSON
    Status,
    /// Link an account by QR code or by pair code
    Link(LinkArgs),
    /// List chats
    Chats,
    /// Print a chat's recent messages as JSON
    History {
        /// Chat JID, for example 15550100@s.whatsapp.net
        jid: Jid,
        /// Number of newest messages to fetch
        #[arg(long, default_value_t = 50)]
        limit: u32,
    },
    /// Send a text message
    Send {
        /// Phone number with country code, or a JID
        to: Recipient,
        /// Message text
        text: String,
    },
    /// Stream incoming messages and reactions as JSON lines
    Watch,
}

#[derive(Args)]
#[command(group(clap::ArgGroup::new("method").required(true).args(["qr", "code"])))]
struct LinkArgs {
    /// Print a QR code to scan from WhatsApp, Linked devices
    #[arg(long)]
    qr: bool,
    /// Request a pair code for this phone number instead of a QR code
    #[arg(long, value_name = "PHONE")]
    code: Option<String>,
}

fn print<T: Serialize>(value: &T) {
    // Serializing our own plain data types cannot fail.
    println!(
        "{}",
        serde_json::to_string(value).unwrap_or_else(|_| "null".into())
    );
}

fn run(cli: Cli) -> Result<()> {
    let wa = Wuzapi::spawn(cli.session.config())?;
    match cli.command {
        Command::Status => {
            let status = wa.status()?;
            let mut value = serde_json::to_value(&status).unwrap_or(Value::Null);
            value["state"] = serde_json::to_value(status.state()).unwrap_or(Value::Null);
            print(&value);
        }
        Command::Link(LinkArgs {
            code: Some(phone), ..
        }) => {
            let pair = wa.link_pair_code(&phone)?;
            print(&json!({ "event": "pair_code", "code": pair.code }));
            match pair.wait_paired(Duration::from_secs(180))? {
                Some(jid) => print(&json!({ "event": "paired", "jid": jid })),
                None => {
                    return Err(Error::PairingFailed(
                        "timed out waiting for the phone".into(),
                    ))
                }
            }
        }
        Command::Link(_) => {
            for step in wa.link_qr()? {
                match step? {
                    LinkEvent::Code(qr) => match qr.render_terminal() {
                        Some(art) => println!("{art}"),
                        None => print(&json!({ "event": "qr", "data_url": qr.data_url })),
                    },
                    LinkEvent::Paired { jid } => {
                        print(&json!({ "event": "paired", "jid": jid }));
                    }
                    LinkEvent::Timeout => {
                        return Err(Error::PairingFailed("QR codes expired; run again".into()))
                    }
                }
            }
        }
        Command::Chats => {
            // Names come from the online session; fall back to JIDs only.
            let _ = wa.connect();
            print(&wa.chats()?);
        }
        Command::History { jid, limit } => print(&wa.history(&jid, limit)?),
        Command::Send { to, text } => {
            wa.connect()?;
            print(&wa.send_text(&to, &text)?);
        }
        Command::Watch => {
            let events = wa.events();
            wa.connect()?;
            for event in events {
                match event {
                    Event::Message { .. } | Event::Reaction { .. } => print(&event),
                    Event::Exited => return Err(Error::Disconnected),
                    _ => {}
                }
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", json!({ "error": e.to_string() }));
            ExitCode::FAILURE
        }
    }
}
