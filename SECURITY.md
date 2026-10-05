# Security

## What is stored

| Item | Where | Notes |
| --- | --- | --- |
| Linked WhatsApp session (device keys) | The data directory (`$XDG_DATA_HOME/whatsapp-link` by default) | Anyone who copies it can read and send messages as you until the device is unlinked. Keep it on storage only you can read. |
| wuzapi user and admin tokens | Derived from your login name unless you set them | They only guard the local wuzapi child. On a shared machine set your own with `WHATSAPP_LINK_USER_TOKEN` and `WHATSAPP_LINK_ADMIN_TOKEN`. |
| Message history | wuzapi's database in the data directory | Users created by this crate keep the last 1000 messages per chat. |

To revoke access, remove the device on your phone (WhatsApp, Settings, Linked devices) or call the library's
`logout()`, then delete the data directory.

Tokens, message bodies and phone numbers are never logged at info level.

## Reporting a vulnerability

Please report security problems privately through GitHub's
[private vulnerability reporting](https://github.com/davidawad/whatsapp-link-rs/security/advisories/new)
instead of opening a public issue. Problems in wuzapi or whatsmeow themselves belong upstream.
