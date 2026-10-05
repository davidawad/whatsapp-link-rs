# whatsapp-link

Link a WhatsApp account and read and send messages from Rust.

`whatsapp-link` is a library (`whatsapp_link`) and a small CLI. It starts
[`wuzapi`](https://github.com/asternic/wuzapi) (a WhatsApp gateway built on
[whatsmeow](https://github.com/tulir/whatsmeow)) as a child process in its stdio
mode, `wuzapi -mode=stdio -datadir=<dir>`, and talks to it with JSON-RPC over the
child's stdin and stdout. Everything the crate exposes is typed: chats, messages,
reactions, link state, events and errors.

> **Terms of service.** Automating WhatsApp through an unofficial client may
> violate WhatsApp's terms of service and can get an account restricted or
> banned. Use a number you can afford to lose, keep volume low, and never use this
> for bulk or unsolicited messaging.
>
> **Unofficial.** Not affiliated with, endorsed by, or supported by WhatsApp or
> Meta. Provided as is, without warranty (see `LICENSE`); you are responsible for
> how you use it. "WhatsApp" is a trademark of its owner, used here only to name
> the service this crate talks to.

## Install

1. Get the `wuzapi` binary.
   * Homebrew (macOS and Linux): `brew install asternic/wuzapi/wuzapi`, or
   * build it from source with Go: `git clone https://github.com/asternic/wuzapi && cd wuzapi && go build -o wuzapi .`
     and put the result on your `PATH` (or pass `--wuzapi-bin`).
   * Check it works: `wuzapi -help` should list `-mode` and `-datadir`.
2. Install the CLI: `cargo install --git https://github.com/davidawad/whatsapp-link-rs`
   (or `cargo install --path .` from a clone), or add `whatsapp-link` as a
   dependency of your own crate. Rust 1.89 or newer is required.

## Link an account

```sh
whatsapp-link status          # {"state":"needs_pairing", ...}
whatsapp-link link --qr       # prints a QR code in the terminal
```

On the phone: WhatsApp, Settings, Linked devices, Link a device, then scan the
code. The QR refreshes by itself; the command prints `{"event":"paired",...}` and
exits when the phone accepts it. If the codes run out, run it again.

No camera handy? Use a pair code instead:

```sh
whatsapp-link link --code +15550100
```

It prints an 8-character code. On the phone choose Linked devices, Link a device,
"Link with phone number instead", and type it in.

From then on the same data directory and user token give you the same session.
By default they live in `$XDG_DATA_HOME/whatsapp-link` (usually
`~/.local/share/whatsapp-link`) and are derived from your login name, so you
normally never think about them again.

## Use it

Output is JSON, one value per line (the QR code is the only exception).

```sh
whatsapp-link status
whatsapp-link chats
whatsapp-link history 15550100@s.whatsapp.net --limit 20
whatsapp-link send +15550100 "hello"
whatsapp-link send 15550100@s.whatsapp.net "hello"
whatsapp-link watch            # incoming messages and reactions, as JSON lines
```

Errors are printed to stderr as `{"error": "..."}` with a non-zero exit code.

### Library

```rust
use whatsapp_link::{Config, Event, Jid, Recipient, Wuzapi};

fn main() -> whatsapp_link::Result<()> {
    let wa = Wuzapi::spawn(Config::from_env())?;   // starts wuzapi, creates the user if needed
    let events = wa.events();                      // subscribe before connecting
    wa.connect()?;

    let chat: Jid = "15550100@s.whatsapp.net".parse()?;
    for m in wa.history(&chat, 20)?.messages {
        println!("{:?}: {:?}", m.sender_name, m.text);
    }

    let sent = wa.send_text(&"+15550100".parse::<Recipient>()?, "hello")?;
    println!("sent {}", sent.id);

    for event in events {
        if let Event::Message { message } = event {
            println!("incoming: {:?}", message.text);
        }
    }
    Ok(())
}
```

* `Wuzapi::spawn(Config)` claims the data directory, starts the child, and
  shuts it down when the value is dropped.
* Linking: `status()`, `link_qr()` (an iterator of QR codes as they refresh),
  `link_pair_code(phone)`, `connect()`, `logout()`.
* Reading: `chats()`, `history(chat, limit)`, `events()`. Reactions are separate
  from messages (`Reaction`, and attached to their target in `History`).
* Writing: `send_text(recipient, body)` returns the new message id.
* One session, one client: if another process already uses the data directory,
  `spawn` fails with `Error::DataDirBusy` instead of running two clients.
* Tokens, message bodies and phone numbers are never logged at info level.

## Configuration

Every setting has a flag and an environment variable; flags win.

| Setting | Flag | Environment variable | Default |
| --- | --- | --- | --- |
| wuzapi binary | `--wuzapi-bin` | `WHATSAPP_LINK_WUZAPI_BIN` | `wuzapi` from `PATH` |
| data directory | `--data-dir` | `WHATSAPP_LINK_DATA_DIR` | `$XDG_DATA_HOME/whatsapp-link` |
| user token | `--user-token` | `WHATSAPP_LINK_USER_TOKEN` | derived from the login name |
| admin token | `--admin-token` | `WHATSAPP_LINK_ADMIN_TOKEN` | derived from the login name |
| timezone | `--timezone` | `WHATSAPP_LINK_TIMEZONE` | `$TZ`, else `UTC` |

## Reuse an existing wuzapi session

Already linked a WhatsApp account through some other wuzapi client? Point this
crate at the same data directory and the same wuzapi user token:

```sh
export WHATSAPP_LINK_DATA_DIR=/path/to/that/wuzapi/datadir
export WHATSAPP_LINK_USER_TOKEN=the-token-that-user-was-created-with
whatsapp-link status           # should report "connected" or "disconnected", not "needs_pairing"
```

Stop the other client first. Two clients on one session fight each other (and can
get the account logged out), so `whatsapp-link` refuses to start while another
process is using the directory, and it takes an exclusive lock of its own.

The wuzapi user must have been created with message history enabled for
`history` to work; users created by this crate keep the last 1000 messages per
chat.

## Cargo features

* `native`: placeholder for a future pure-Rust backend (for example the
  [`whatsapp-rust`](https://github.com/oxidezap/whatsapp-rust) crate, MIT) that
  would replace the wuzapi child process. It currently does nothing.

## Tests

`cargo test` needs no network and no WhatsApp account. It runs against an
in-process fake of wuzapi's JSON-RPC peer and a shell-script stand-in for the
binary, using synthetic data only.

## Security

The data directory holds a live, linked WhatsApp session. See
[SECURITY.md](SECURITY.md) for what is stored and how to report a vulnerability.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

GPL-3.0-only. See `LICENSE`. wuzapi (MIT) and whatsmeow (MPL-2.0) are not
linked into this crate; it runs the `wuzapi` binary as a separate process.
