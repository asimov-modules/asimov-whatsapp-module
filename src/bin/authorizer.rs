// This is free and unencumbered software released into the public domain.

use clientele::{
    crates::clap::{self, Parser},
    StandardOptions,
    SysexitsError::{self, *},
};
use miette::{miette, IntoDiagnostic as _, Result};

use qrcode::render::unicode::Dense1x2;
use qrcode::QrCode;

use std::env;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use whatsapp_rust::bot::Bot as WhatsApp;
use whatsapp_rust::pair_code::PairCodeOptions;
use whatsapp_rust::store::SqliteStore;
use whatsapp_rust::transport::{TokioWebSocketTransportFactory, UreqHttpClient};
use whatsapp_rust::types::events::Event;

#[derive(Debug, Parser)]
#[command(name = "asimov-whatsapp-authorizer", long_about)]
struct Options {
    #[clap(flatten)]
    flags: StandardOptions,

    /// Path to the SQLite DB. If omitted, uses WA_DB env var.
    #[arg(long, value_name = "PATH")]
    db: Option<PathBuf>,

    /// If set, authorizes via Pair Code (instead of QR).
    #[arg(short, long)]
    phone: Option<String>,

    /// Login wait timeout in seconds.
    #[arg(long, default_value_t = 180)]
    timeout: u64,

    /// Delete the DB file (if it exists) and exit.
    #[arg(long)]
    reset: bool,

    /// Print DB path + whether it exists, then exit.
    #[arg(long)]
    status: bool,
}

#[derive(Copy, Clone, Debug)]
enum AuthMode {
    QrOnly,
    PairCodeOnly,
}

impl AuthMode {
    fn from_options(opts: &Options) -> Self {
        if opts.phone.is_some() {
            Self::PairCodeOnly
        } else {
            Self::QrOnly
        }
    }
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

fn ensure_parent_dir(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).into_diagnostic()?;
        }
    }
    Ok(())
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

fn render_qr_to_stdout(payload: &str) {
    match QrCode::new(payload.as_bytes()) {
        Ok(qr) => {
            let rendered = qr.render::<Dense1x2>().quiet_zone(true).build();
            println!("{rendered}");
            let _ = std::io::stdout().flush();
        }
        Err(e) => {
            eprintln_flush(format!("ERROR: failed to render QR: {e}"));
            println!("QR_PAYLOAD:\n{payload}");
            let _ = std::io::stdout().flush();
        }
    }
}

fn reset_db(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path).into_diagnostic()?;
    }
    Ok(())
}

async fn run(options: Options) -> Result<SysexitsError> {
    if options.flags.version {
        println!("asimov-whatsapp-authorizer {}", env!("CARGO_PKG_VERSION"));
        return Ok(EX_OK);
    }

    let db = match resolve_db_path(options.db.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln_flush(format!("ERROR: {e}"));
            return Ok(EX_USAGE);
        }
    };

    if let Err(e) = ensure_parent_dir(&db) {
        eprintln_flush(format!("ERROR: failed to create db parent dir: {e}"));
        return Ok(EX_SOFTWARE);
    }

    if options.reset {
        if let Err(e) = reset_db(&db) {
            eprintln_flush(format!("ERROR: failed to reset db: {e}"));
            return Ok(EX_SOFTWARE);
        }
        return Ok(EX_OK);
    }

    if options.status {
        println!("database: {}", db.display());
        println!("configured: {}", db.exists());
        return Ok(if db.exists() { EX_OK } else { EX_NOINPUT });
    }

    if let Some(phone) = &options.phone {
        if let Err(e) = validate_phone_digits(phone) {
            eprintln_flush(format!("ERROR: {e}"));
            return Ok(EX_USAGE);
        }
    }

    let auth_mode = AuthMode::from_options(&options);
    let timeout = Duration::from_secs(options.timeout);

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

    let mut builder = WhatsApp::builder()
        .with_backend(backend)
        .with_transport_factory(TokioWebSocketTransportFactory::new())
        .with_http_client(UreqHttpClient::new())
        .on_event(move |event, _client| {
            let auth_mode = auth_mode;
            async move {
                match (auth_mode, event) {
                    (AuthMode::QrOnly, Event::PairingQrCode { code, .. }) => {
                        render_qr_to_stdout(&code);
                    }
                    (AuthMode::PairCodeOnly, Event::PairingCode { code, timeout }) => {
                        println!("PAIR CODE: {code}");
                        println!("PAIR CODE TIMEOUT (seconds): {}", timeout.as_secs());
                        let _ = std::io::stdout().flush();
                    }
                    _ => {}
                }
            }
        });

    if let Some(phone) = &options.phone {
        let mut pc = PairCodeOptions {
            phone_number: phone.clone(),
            ..Default::default()
        };
        if pc.platform_display.trim().is_empty() {
            pc.platform_display = "Asimov (Rust)".to_string();
        }
        builder = builder.with_pair_code(pc);
    }

    let mut wa = match builder.build().await {
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

    let authorized = match client.wait_for_connected(timeout).await {
        Ok(()) => true,
        Err(e) => {
            eprintln_flush(format!("ERROR: failed while waiting for login: {e}"));
            false
        }
    };

    println!("AUTHORIZED: {}", if authorized { "true" } else { "false" });
    let _ = std::io::stdout().flush();

    let _ = client.disconnect().await;
    run_handle.abort();

    Ok(if authorized { EX_OK } else { EX_TEMPFAIL })
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
