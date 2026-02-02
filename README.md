# ASIMOV WhatsApp Module

[![License](https://img.shields.io/badge/license-Public%20Domain-blue.svg)](https://unlicense.org)
[![Package on Crates.io](https://img.shields.io/crates/v/asimov-whatsapp-module)](https://crates.io/crates/asimov-whatsapp-module)
[![Documentation](https://docs.rs/asimov-whatsapp-module/badge.svg)](https://docs.rs/asimov-whatsapp-module)

[ASIMOV] module for [WhatsApp]. Data import for WhatsApp account!

## ✨ Features

- To be determined!

## 🛠️ Prerequisites

- [Rust] 1.85+ (2024 edition) if building from source code

## ⬇️ Installation

### Installation with the [ASIMOV CLI]

```bash
asimov module install whatsapp -v
```

### Installation from Source Code

```bash
cargo install asimov-whatsapp-module
```

## 👉 Examples

`asimov-whatsapp-authorizer` - authorize WhatsApp account via QR code or pair code (phone number).

Check status:
```bash
asimov-whatsapp-authorizer --status
```

Reset local DB:
```bash
asimov-whatsapp-authorizer --reset
```

Authorize via QR code:Authorize via QR code:
```bash
asimov-whatsapp-authorizer
```

Authorize via pair code (phone number):
```bash
asimov-whatsapp-authorizer --phone 15551234567
```

`asimov-whatsapp-cataloger` - list all WhatsApp groups and members.

List all groups:
```bash
asimov-whatsapp-cataloger wa:groups
```

List members of a group:
```bash
asimov-whatsapp-cataloger wa:group/<GROUP_JID>/members
```

`asimov-whatsapp-fetcher` - fetch WhatsApp contacts and groups metadata.

Fetch group metadata:
```bash
asimov-whatsapp-fetcher wa:group/<GROUP_JID>
```

Fetch contact by phone number:
```bash
asimov-whatsapp-fetcher wa:contact/<PHONE_DIGITS>
```

Fetch contact by JID (LID or phone JID):
```bash
asimov-whatsapp-fetcher wa:contact-jid/<JID>
```

`asimov-whatsapp-emitter` - stream incoming WhatsApp messages to STDOUT as JSONL.

By default emits only inbound (messages sent to you). Use `--outbound` to also include outbound (messages sent by you).
By default emits a minimal JSON shape for text-like messages only. Use `--full` to emit the full raw payload.

Examples:

Minimal (inbound only):
```bash
asimov-whatsapp-emitter | jq .
```

Minimal + include outbound:
```bash
asimov-whatsapp-emitter --outbound | jq .
```

Full payload (message + message_info):
```bash
asimov-whatsapp-emitter --full | jq .
```

## ⚙ Configuration

All scripts read the database path from WA_DB.
```bash
export WA_DB="$PWD/whatsapp.db"
```

## 📚 Reference

### Installed Binaries

- `asimov-whatsapp-authorizer`
- `asimov-whatsapp-cataloger`
- `asimov-whatsapp-emitter`
- `asimov-whatsapp-fetcher`

### `asimov-whatsapp-authorizer`

```
asimov-whatsapp-authorizer

Usage: asimov-whatsapp-authorizer [OPTIONS]

Options:
  -d, --debug              Enable debugging output
      --license            Show license information
  -v, --verbose...         Enable verbose output (may be repeated for more verbosity)
  -V, --version            Print version information
      --db <PATH>          Path to the SQLite DB. If omitted, uses WA_DB env var
  -p, --phone <PHONE>      If set, authorizes via Pair Code (instead of QR)
      --timeout <TIMEOUT>  Login wait timeout in seconds [default: 180]
      --reset              Delete the DB file (if it exists) and exit
      --status             Print DB path + whether it exists, then exit
  -h, --help               Print help
```

### `asimov-whatsapp-cataloger`

```
asimov-whatsapp-cataloger

Usage: asimov-whatsapp-cataloger [OPTIONS] <RESOURCE>

Arguments:
  <RESOURCE>  Resource URL (e.g. wa:groups, wa:group/<jid>/members)

Options:
  -d, --debug              Enable debugging output
      --license            Show license information
  -v, --verbose...         Enable verbose output (may be repeated for more verbosity)
  -V, --version            Print version information
      --db <PATH>          Path to the SQLite DB. If omitted, uses WA_DB env var
  -n, --limit <COUNT>      Maximum number of items to output
      --timeout <TIMEOUT>  Connection wait timeout in seconds [default: 30]
  -h, --help               Print help
```

### `asimov-whatsapp-emitter`

```
asimov-whatsapp-emitter

Usage: asimov-whatsapp-emitter [OPTIONS]

Options:
  -d, --debug              Enable debugging output
      --license            Show license information
  -v, --verbose...         Enable verbose output (may be repeated for more verbosity)
  -V, --version            Print version information
      --db <PATH>          Path to the SQLite DB. If omitted, uses WA_DB env var
      --full               Print full payload (message + message_info) instead of minimal shape
      --outbound           Include outbound (messages from me). Inbound is always included
      --timeout <TIMEOUT>  Connection wait timeout in seconds [default: 30]
  -h, --help               Print help
```

### `asimov-whatsapp-emitter`

```
asimov-whatsapp-fetcher

Usage: asimov-whatsapp-fetcher [OPTIONS] <RESOURCE>

Arguments:
  <RESOURCE>  Resource URL (e.g. wa:group/<jid>, wa:contact/<digits>, wa:contact-jid/<jid>)

Options:
  -d, --debug              Enable debugging output
      --license            Show license information
  -v, --verbose...         Enable verbose output (may be repeated for more verbosity)
  -V, --version            Print version information
      --db <PATH>          Path to the SQLite DB. If omitted, uses WA_DB env var
      --timeout <TIMEOUT>  Connection wait timeout in seconds [default: 30]
  -h, --help               Print help
```

## 👨‍💻 Development

```bash
git clone https://github.com/asimov-modules/asimov-whatsapp-module.git
```

---

[![Share on X](https://img.shields.io/badge/share%20on-x-03A9F4?logo=x)](https://x.com/intent/post?url=https://github.com/asimov-modules/asimov-whatsapp-module&text=asimov-whatsapp-module)
[![Share on Reddit](https://img.shields.io/badge/share%20on-reddit-red?logo=reddit)](https://reddit.com/submit?url=https://github.com/asimov-modules/asimov-whatsapp-module&title=asimov-whatsapp-module)
[![Share on Hacker News](https://img.shields.io/badge/share%20on-hn-orange?logo=ycombinator)](https://news.ycombinator.com/submitlink?u=https://github.com/asimov-modules/asimov-whatsapp-module&t=asimov-whatsapp-module)
[![Share on Facebook](https://img.shields.io/badge/share%20on-fb-1976D2?logo=facebook)](https://www.facebook.com/sharer/sharer.php?u=https://github.com/asimov-modules/asimov-whatsapp-module)
[![Share on LinkedIn](https://img.shields.io/badge/share%20on-linkedin-3949AB?logo=linkedin)](https://www.linkedin.com/sharing/share-offsite/?url=https://github.com/asimov-modules/asimov-whatsapp-module)

[ASIMOV]: https://asimov.sh
[ASIMOV CLI]: https://cli.asimov.sh
[Rust]: https://rust-lang.org
[WhatsApp]: https://www.whatsapp.com
