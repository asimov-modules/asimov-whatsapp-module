// This is free and unencumbered software released into the public domain.

use clientele::{
    StandardOptions,
    SysexitsError::{self, *},
    crates::clap::{self, Parser},
};
use miette::{IntoDiagnostic as _, Result, miette};

use serde_json::{Value as JsonValue, json};
use std::collections::HashSet;
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
#[command(
    name = "asimov-whatsapp-fetcher",
    disable_help_subcommand = true,
    long_about
)]
struct Options {
    #[clap(flatten)]
    flags: StandardOptions,

    /// Path to the SQLite DB. If omitted, uses WA_DB env var.
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,

    /// Connection wait timeout in seconds.
    #[arg(long, default_value_t = 30)]
    timeout: u64,

    /// Resource URL (e.g. wa:group/<jid>, wa:contact/<digits>, wa:contact-jid/<jid>)
    resource: String,
}

#[derive(Clone, Debug)]
enum Target {
    Group { group_jid: Jid },
    ContactByPhone { phone: String },
    ContactByJid { jid: Jid },
}

fn eprintln_flush(msg: impl AsRef<str>) {
    eprintln!("{}", msg.as_ref());
    let _ = std::io::stderr().flush();
}

fn stdout_json_line(v: &serde_json::Value) {
    println!(
        "{}",
        serde_json::to_string(v).expect("failed to serialize JSON")
    );
    let _ = std::io::stdout().flush();
}

fn resolve_db_path(db: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(p) = db {
        return Ok(p);
    }
    match env::var("WA_DB") {
        Ok(v) if !v.trim().is_empty() => Ok(PathBuf::from(v)),
        _ => Err(miette!(
            "missing DB path: pass --db PATH or set WA_DB env var"
        )),
    }
}

fn validate_phone_digits(phone: &str) -> Result<()> {
    let p = phone.trim();
    if p.is_empty() {
        return Err(miette!("phone must not be empty"));
    }
    if !p.chars().all(|c| c.is_ascii_digit()) {
        return Err(miette!("phone must contain digits only (got: '{phone}')"));
    }
    if p.len() < 8 || p.len() > 20 {
        return Err(miette!(
            "phone length looks wrong (expected 8..20 digits, got {})",
            p.len()
        ));
    }
    Ok(())
}

fn parse_resource_url(s: &str) -> Result<Target> {
    let s = s.trim();
    let s = s
        .strip_prefix("wa:")
        .ok_or_else(|| miette!("resource must start with 'wa:'"))?;

    if let Some(rest) = s.strip_prefix("group/") {
        let group_jid: Jid = rest.trim().parse().into_diagnostic()?;
        return Ok(Target::Group { group_jid });
    }

    if let Some(phone) = s.strip_prefix("contact/") {
        let phone = phone.trim().to_string();
        validate_phone_digits(&phone)?;
        return Ok(Target::ContactByPhone { phone });
    }

    if let Some(jid_str) = s.strip_prefix("contact-jid/") {
        let jid: Jid = jid_str.trim().parse().into_diagnostic()?;
        return Ok(Target::ContactByJid { jid });
    }

    Err(miette!("unsupported resource: {s}"))
}

fn dedup_jids(mut v: Vec<Jid>) -> Vec<Jid> {
    let mut seen = HashSet::<String>::new();
    v.retain(|jid| seen.insert(jid.to_string()));
    v
}

async fn fetch_user_info_and_pfp_best_effort(
    client: &whatsapp_rust::Client,
    candidates: &[Jid],
) -> (Option<JsonValue>, Option<JsonValue>, Vec<String>) {
    let mut errors: Vec<String> = Vec::new();

    let mut user_info_json: Option<JsonValue> = None;
    for jid in candidates {
        match client.contacts().get_user_info(&[jid.clone()]).await {
            Ok(mut map) => {
                if let Some(u) = map.remove(jid) {
                    user_info_json = Some(json!({
                        "jid": u.jid.to_string(),
                        "lid": u.lid.as_ref().map(|x| x.to_string()),
                        "status": u.status,
                        "picture_id": u.picture_id,
                        "is_business": u.is_business,
                    }));
                    break;
                }
            },
            Err(e) => errors.push(format!("contacts.get_user_info({jid}) error: {e}")),
        }
    }

    let mut pfp_json: Option<JsonValue> = None;
    for jid in candidates {
        match client.contacts().get_profile_picture(jid, true).await {
            Ok(Some(p)) => {
                pfp_json = Some(json!({
                    "id": p.id,
                    "url": p.url,
                    "direct_path": p.direct_path,
                }));
                break;
            },
            Ok(None) => {},
            Err(e) => errors.push(format!("contacts.get_profile_picture({jid}) error: {e}")),
        }
    }

    (user_info_json, pfp_json, errors)
}

async fn run(options: Options) -> Result<SysexitsError> {
    if options.flags.version {
        println!("asimov-whatsapp-fetcher {}", env!("CARGO_PKG_VERSION"));
        return Ok(EX_OK);
    }

    let db = match resolve_db_path(options.db.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln_flush(format!("ERROR: {e}"));
            return Ok(EX_USAGE);
        },
    };

    let target = match parse_resource_url(&options.resource) {
        Ok(t) => t,
        Err(e) => {
            eprintln_flush(format!("ERROR: {e}"));
            return Ok(EX_USAGE);
        },
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
        },
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
        },
    };

    let client = wa.client();

    let run_handle = match wa.run().await {
        Ok(h) => h,
        Err(e) => {
            eprintln_flush(format!("ERROR: failed to start WA runtime: {e}"));
            return Ok(EX_SOFTWARE);
        },
    };

    let timeout = Duration::from_secs(options.timeout);
    if let Err(e) = client.wait_for_connected(timeout).await {
        eprintln_flush(format!("ERROR: unauthorized / not connected: {e}"));
        run_handle.abort();
        return Ok(EX_NOPERM);
    }

    let rc = match target {
        Target::Group { group_jid } => match client.groups().get_metadata(&group_jid).await {
            Ok(meta) => {
                stdout_json_line(&json!({
                    "kind": "group",
                    "jid": meta.id.to_string(),
                    "subject": meta.subject,
                    "addressing_mode": format!("{:?}", meta.addressing_mode),
                    "participants_count": meta.participants.len(),
                    "participants": meta.participants.into_iter().map(|p| json!({
                        "jid": p.jid.to_string(),
                        "phone_jid": p.phone_number.as_ref().map(|j| j.to_string()),
                        "is_admin": p.is_admin,
                    })).collect::<Vec<_>>(),
                }));
                EX_OK
            },
            Err(e) => {
                eprintln_flush(format!(
                    "ERROR: groups.get_metadata({group_jid}) failed: {e}"
                ));
                EX_SOFTWARE
            },
        },

        Target::ContactByPhone { phone } => {
            match client.contacts().get_info(&[phone.as_str()]).await {
                Ok(list) => {
                    if let Some(info) = list.into_iter().next() {
                        let mut candidates: Vec<Jid> = Vec::new();

                        if let Some(lid) = info.lid.clone() {
                            candidates.push(lid);
                        }

                        candidates.push(info.jid.clone());

                        let phone_jid_str = format!("{phone}@s.whatsapp.net");
                        if let Ok(phone_jid) = phone_jid_str.parse::<Jid>() {
                            candidates.push(phone_jid);
                        }

                        let candidates = dedup_jids(candidates);
                        let (ui, pfp, errs) =
                            fetch_user_info_and_pfp_best_effort(client.as_ref(), &candidates).await;

                        stdout_json_line(&json!({
                            "kind": "contact",
                            "query": { "phone": phone },
                            "contact_info": {
                                "jid": info.jid.to_string(),
                                "lid": info.lid.as_ref().map(|j| j.to_string()),
                                "is_registered": info.is_registered,
                                "is_business": info.is_business,
                                "status": info.status,
                                "picture_id": info.picture_id,
                            },
                            "user_info": ui,
                            "profile_picture_preview": pfp,
                            "errors": errs,
                        }));
                        EX_OK
                    } else {
                        stdout_json_line(&json!({
                            "kind": "contact",
                            "query": { "phone": phone },
                            "found": false,
                            "errors": ["contacts.get_info returned empty result"],
                        }));
                        EX_OK
                    }
                },
                Err(e) => {
                    stdout_json_line(&json!({
                        "kind": "contact",
                        "query": { "phone": phone },
                        "found": false,
                        "errors": [format!("contacts.get_info error: {e}")],
                    }));
                    EX_OK
                },
            }
        },

        Target::ContactByJid { jid } => {
            let (ui, pfp, errs) =
                fetch_user_info_and_pfp_best_effort(client.as_ref(), &[jid.clone()]).await;

            stdout_json_line(&json!({
                "kind": "contact",
                "query": { "jid": jid.to_string() },
                "user_info": ui,
                "profile_picture_preview": pfp,
                "errors": errs,
            }));
            EX_OK
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
