// This is free and unencumbered software released into the public domain.

use clientele::{
    crates::clap::{self, Parser},
    StandardOptions,
    SysexitsError::{self, *},
};
use miette::{miette, IntoDiagnostic as _, Result};

use serde_json::json;

use std::env;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use whatsapp_rust::bot::Bot as WhatsApp;
use whatsapp_rust::store::SqliteStore;
use whatsapp_rust::transport::{TokioWebSocketTransportFactory, UreqHttpClient};

use wacore_binary::jid::Jid;

#[derive(Debug, Parser)]
#[command(name = "asimov-whatsapp-cataloger", long_about)]
struct Options {
    #[clap(flatten)]
    flags: StandardOptions,

    /// Path to the SQLite DB. If omitted, uses WA_DB env var.
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,

    /// Maximum number of items to output.
    #[arg(value_name = "COUNT", short = 'n', long)]
    limit: Option<usize>,

    /// Connection wait timeout in seconds.
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Resource URL (e.g. wa:groups, wa:group/<jid>/members)
    resource: String,
}

#[derive(Clone, Debug)]
enum Target {
    Groups,
    GroupMembers { group_jid: Jid },
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

fn parse_resource_url(s: &str) -> Result<Target> {
    let s = s.trim();
    let s = s
        .strip_prefix("wa:")
        .ok_or_else(|| miette!("resource must start with 'wa:'"))?;

    if s == "groups" {
        return Ok(Target::Groups);
    }

    if let Some(rest) = s.strip_prefix("group/") {
        let mut parts = rest.split('/');
        let jid_str = parts
            .next()
            .ok_or_else(|| miette!("expected wa:group/<jid>/members"))?;
        let tail = parts.next().unwrap_or("");
        if tail != "members" {
            return Err(miette!("expected wa:group/<jid>/members"));
        }
        let group_jid: Jid = jid_str.parse().into_diagnostic()?;
        return Ok(Target::GroupMembers { group_jid });
    }

    Err(miette!("unsupported resource: {s}"))
}

fn stdout_json_line(v: &serde_json::Value) {
    println!("{}", serde_json::to_string(v).expect("failed to serialize JSON"));
    let _ = std::io::stdout().flush();
}

async fn run(options: Options) -> Result<SysexitsError> {
    if options.flags.version {
        println!("asimov-whatsapp-cataloger {}", env!("CARGO_PKG_VERSION"));
        return Ok(EX_OK);
    }

    let db = match resolve_db_path(options.db.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln_flush(format!("ERROR: {e}"));
            return Ok(EX_USAGE);
        }
    };

    let target = match parse_resource_url(&options.resource) {
        Ok(t) => t,
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

    let mut wa = match WhatsApp::builder()
        .with_backend(backend)
        .with_transport_factory(TokioWebSocketTransportFactory::new())
        .with_http_client(UreqHttpClient::new())
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

    let run_handle = match wa.run().await {
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

    let limit = options.limit.unwrap_or(usize::MAX);

    let rc = match target {
        Target::Groups => match client.groups().get_participating().await {
            Ok(groups) => {
                let mut items: Vec<_> = groups.into_iter().map(|(_k, meta)| meta).collect();
                items.sort_by(|a, b| {
                    a.subject
                        .cmp(&b.subject)
                        .then_with(|| a.id.to_string().cmp(&b.id.to_string()))
                });

                for meta in items.into_iter().take(limit) {
                    let out = json!({
                        "kind": "group",
                        "jid": meta.id.to_string(),
                        "subject": meta.subject,
                        "addressing_mode": format!("{:?}", meta.addressing_mode),
                        "participants_count": meta.participants.len(),
                        "members_resource": format!("wa:group/{}/members", meta.id),
                    });
                    stdout_json_line(&out);
                }

                EX_OK
            }
            Err(e) => {
                eprintln_flush(format!("ERROR: groups.get_participating failed: {e}"));
                EX_SOFTWARE
            }
        },

        Target::GroupMembers { group_jid } => match client.groups().get_metadata(&group_jid).await {
            Ok(meta) => {
                for p in meta.participants.into_iter().take(limit) {
                    let out = json!({
                        "kind": "group_member",
                        "group_jid": meta.id.to_string(),
                        "group_subject": meta.subject,
                        "member_jid": p.jid.to_string(),
                        "member_phone_jid": p.phone_number.as_ref().map(|j| j.to_string()),
                        "is_admin": p.is_admin,
                        "contact_fetch_by_lid": format!("wa:contact-jid/{}", p.jid),
                        "contact_fetch_by_phone_jid": p.phone_number.as_ref().map(|j| format!("wa:contact-jid/{j}")),
                    });
                    stdout_json_line(&out);
                }

                EX_OK
            }
            Err(e) => {
                eprintln_flush(format!("ERROR: groups.get_metadata({group_jid}) failed: {e}"));
                EX_SOFTWARE
            }
        },
    };

    let _ = client.disconnect().await;
    run_handle.abort();

    Ok(rc)
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
