# Contributing

## Setup

```sh
git clone https://github.com/davidawad/whatsapp-link-rs && cd whatsapp-link-rs
just ci        # or: make ci   (fmt --check, clippy -D warnings, check, test)
```

`cargo test` needs no network, no `wuzapi` binary and no WhatsApp account. It runs against an in-process fake
of wuzapi's JSON-RPC peer and a shell-script stand-in for the binary.

## Rules

- Commit messages follow [Conventional Commits](https://www.conventionalcommits.org/); the changelog is
  generated from them.
- Synthetic data only in tests, fixtures, docs and issues: no real phone numbers, JIDs, contact or group
  names, tokens or message text. Use `+15550100`, `1@s.whatsapp.net`, `999@g.us` and the like.
- Never commit a wuzapi data directory or database.
- Don't add tests that talk to real WhatsApp. If you change the link flow, try it by hand with a throwaway
  account and say so in the PR.
- Keep the public API typed: no stringly JSON in signatures.
