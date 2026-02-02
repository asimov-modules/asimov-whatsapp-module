// This is free and unencumbered software released into the public domain.

use clientele::{
    crates::clap::{self, Parser},
    StandardOptions,
    SysexitsError::{self, *},
};
use miette::{miette, Result};

use serde::Serialize;
use serde_json::json;

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use whatsapp_rust::bot::Bot as WhatsApp;
use whatsapp_rust::store::SqliteStore;
use whatsapp_rust::transport::{TokioWebSocketTransportFactory, UreqHttpClient};
use whatsapp_rust::types::events::Event;
use whatsapp_rust::types::message::MessageInfo;

use waproto::whatsapp::Message;

#[derive(Debug, Parser)]
#[command(name = "asimov-whatsapp-emitter", long_about)]
struct Options {
    #[clap(flatten)]
    flags: StandardOptions,

    /// Path to the SQLite DB. If omitted, uses WA_DB env var.
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,

    /// Print full payload (message + message_info) instead of minimal shape.
    #[arg(long)]
    full: bool,

    /// Include outbound (messages from me). Inbound is always included.
    #[arg(long)]
    outbound: bool,

    /// Connection wait timeout in seconds.
    #[arg(long, default_value_t = 30)]
    timeout: u64,
}

fn eprintln_flush(msg: impl AsRef<str>) {
    eprintln!("{}", msg.as_ref());
    let _ = std::io::stderr().flush();
}

fn resolve_db_path(db: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = db {
        return Ok(p);
    }
    match env::var("WA_DB") {
        Ok(v) if !v.trim().is_empty() => Ok(PathBuf::from(v)),
        _ => Err(miette!("missing DB path: pass --db PATH or set WA_DB env var")),
    }
}

fn stdout_json_line<T: Serialize>(v: &T) {
    println!("{}", serde_json::to_string(v).expect("failed to serialize JSON"));
    let _ = std::io::stdout().flush();
}

fn unwrap_message(mut msg: &Message) -> &Message {
    for _ in 0..16 {
        if let Some(dsm) = msg.device_sent_message.as_ref() {
            if let Some(inner) = dsm.message.as_ref() {
                msg = inner;
                continue;
            }
        }
        if let Some(eph) = msg.ephemeral_message.as_ref() {
            if let Some(inner) = eph.message.as_ref() {
                msg = inner;
                continue;
            }
        }
        if let Some(vo) = msg.view_once_message.as_ref() {
            if let Some(inner) = vo.message.as_ref() {
                msg = inner;
                continue;
            }
        }
        if let Some(vo2) = msg.view_once_message_v2.as_ref() {
            if let Some(inner) = vo2.message.as_ref() {
                msg = inner;
                continue;
            }
        }
        if let Some(vo2x) = msg.view_once_message_v2_extension.as_ref() {
            if let Some(inner) = vo2x.message.as_ref() {
                msg = inner;
                continue;
            }
        }
        break;
    }
    msg
}

fn non_empty(s: Option<&str>) -> Option<&str> {
    s.map(str::trim).filter(|s| !s.is_empty())
}

fn extract_text(core: &Message) -> Option<String> {
    if let Some(s) = non_empty(core.conversation.as_deref()) {
        return Some(s.to_string());
    }
    if let Some(etm) = core.extended_text_message.as_ref() {
        if let Some(s) = non_empty(etm.text.as_deref()) {
            return Some(s.to_string());
        }
    }
    None
}

#[derive(Debug, Serialize)]
struct OutChat {
    is_group: bool,
    jid: String,
}

#[derive(Debug, Serialize)]
struct OutSender {
    from_me: bool,
    jid: String,
    alt_jid: Option<String>,
    push_name: Option<String>,
}

#[derive(Debug, Serialize)]
struct OutRouting {
    addressing_mode: Option<String>,
    recipient_jid: Option<String>,
}

#[derive(Debug, Serialize)]
struct OutContent {
    #[serde(rename = "type")]
    ty: &'static str,
    text: String,
}

#[derive(Debug, Serialize)]
struct OutMinimal {
    kind: &'static str,
    id: String,
    timestamp: String,
    chat: OutChat,
    sender: OutSender,
    routing: OutRouting,
    content: OutContent,
}

fn to_minimal(msg: &Message, info: &MessageInfo) -> Option<OutMinimal> {
    let core = unwrap_message(msg);
    let text = extract_text(core)?;

    let src = &info.source;

    let chat = OutChat {
        is_group: src.is_group,
        jid: src.chat.to_string(),
    };

    let sender = OutSender {
        from_me: src.is_from_me,
        jid: src.sender.to_string(),
        alt_jid: src.sender_alt.as_ref().map(|j| j.to_string()),
        push_name: non_empty(Some(info.push_name.as_str())).map(|s| s.to_string()),
    };

    let routing = OutRouting {
        addressing_mode: src.addressing_mode.as_ref().map(|m| format!("{m:?}")),
        recipient_jid: src.recipient.as_ref().map(|j| j.to_string()),
    };

    Some(OutMinimal {
        kind: "message",
        id: info.id.clone(),
        timestamp: info.timestamp.to_string(),
        chat,
        sender,
        routing,
        content: OutContent { ty: "text", text },
    })
}

async fn run(options: Options) -> Result<SysexitsError> {
    if options.flags.version {
        println!("asimov-whatsapp-emitter {}", env!("CARGO_PKG_VERSION"));
        return Ok(EX_OK);
    }

    let db = match resolve_db_path(options.db.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln_flush(format!("ERROR: {e}"));
            return Ok(EX_USAGE);
        }
    };

    if !db.exists() {
        eprintln_flush(format!(
            "ERROR: not configured (db missing: {}). Run authorizer first.",
            db.display()
        ));
        return Ok(EX_NOINPUT);
    }

    let backend = match SqliteStore::new(db.to_string_lossy().as_ref()).await {
        Ok(s) => Arc::new(s),
        Err(e) => {
            eprintln_flush(format!(
                "ERROR: failed to open sqlite store at {}: {e}",
                db.display()
            ));
            return Ok(EX_SOFTWARE);
        }
    };

    let include_outbound = options.outbound;
    let full = options.full;

    let mut wa = match WhatsApp::builder()
        .with_backend(backend)
        .with_transport_factory(TokioWebSocketTransportFactory::new())
        .with_http_client(UreqHttpClient::new())
        .on_event(move |event, _client| async move {
            if let Event::Message(msg, info) = event {
                if info.source.is_from_me && !include_outbound {
                    return;
                }

                if full {
                    stdout_json_line(&json!({
                        "kind": "message",
                        "message": &*msg,
                        "message_info": info,
                    }));
                    return;
                }

                if let Some(out) = to_minimal(&msg, &info) {
                    stdout_json_line(&out);
                }
            }
        })
        .build()
        .await
    {
        Ok(b) => b,
        Err(e) => {
            eprintln_flush(format!("ERROR: failed to build WA runtime: {e}"));
            return Ok(EX_SOFTWARE);
        }
    };

    let client = wa.client();

    let mut run_handle = match wa.run().await {
        Ok(h) => h,
        Err(e) => {
            eprintln_flush(format!("ERROR: failed to start WA runtime: {e}"));
            return Ok(EX_SOFTWARE);
        }
    };

    let timeout = Duration::from_secs(options.timeout);
    if let Err(e) = client.wait_for_connected(timeout).await {
        eprintln_flush(format!("ERROR: unauthorized / not connected: {e}"));
        run_handle.abort();
        return Ok(EX_NOPERM);
    }

    tokio::select! {
        _ = tokio::signal::ctrl_c() => { }
        _ = &mut run_handle => {
            eprintln_flush("ERROR: WA runtime ended unexpectedly");
            return Ok(EX_SOFTWARE);
        }
    }

    let _ = client.disconnect().await;
    Ok(EX_OK)
}

#[tokio::main]
async fn main() -> Result<SysexitsError> {
    // Load environment variables from `.env`:
    clientele::dotenv().ok();

    // Expand wildcards and @argfiles:
    let Ok(args) = clientele::args_os() else {
        return Ok(EX_USAGE);
    };

    // Parse command-line options:
    let options = Options::parse_from(&args);

    asimov_module::init_tracing_subscriber(&options.flags)
        .map_err(|e| miette!("failed to initialize logging: {e}"))?;

    run(options).await
}
