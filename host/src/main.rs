use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use companion_host::app;
use companion_host::config::{Config, Paths};
use companion_host::net::message::ByeReason;
use companion_host::net::server::{self, Listeners};
use companion_host::state::AppState;
#[cfg(feature = "tray")]
use companion_host::tray;
use companion_host::widgets::layout::Layout;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

const USAGE: &str = "\
companion-host: PC side of PSP Companion

Usage: companion-host [--headless] [--pair] [--config <file>]

  --headless       run without the tray icon (stop with Ctrl+C)
  --pair           open pairing at start-up and log the PIN
  --config <file>  use this config file instead of the per-user default
";

struct Args {
    headless: bool,
    pair: bool,
    config: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args { headless: false, pair: false, config: None };
    let mut it = std::env::args().skip(1);
    while let Some(a) = it.next() {
        match a.as_str() {
            "--headless" => args.headless = true,
            "--pair" => args.pair = true,
            "--config" => args.config = Some(it.next().ok_or("--config needs a file path")?.into()),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(a) => a,
        Err(msg) => {
            if !msg.is_empty() {
                eprintln!("{msg}\n");
            }
            eprint!("{USAGE}");
            return if msg.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(2) };
        }
    };
    let paths = match Paths::discover(args.config.clone()) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    let file_appender = tracing_appender::rolling::daily(&paths.log_dir, "companion-host.log");
    let (file_writer, _log_guard) = tracing_appender::non_blocking(file_appender);
    tracing_subscriber::registry()
        .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
                .with_writer(std::io::stderr),
        )
        .with(tracing_subscriber::fmt::layer().with_ansi(false).with_writer(file_writer))
        .init();

    match run(args, paths) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            error!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: Args, paths: Paths) -> Result<(), String> {
    let config = Config::load_or_create(&paths.config_file).map_err(|e| e.to_string())?;
    info!(config = %paths.config_file.display(), logs = %paths.log_dir.display(), "starting companion-host {}", server::SW_VERSION);

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .map_err(|e| format!("cannot start async runtime: {e}"))?;

    let state = AppState::new(config.clone(), paths.config_file.clone());
    let listeners = runtime
        .block_on(Listeners::bind(&config.network))
        .map_err(|e| format!("cannot bind network ports (is another instance running?): {e}"))?;
    info!(
        discovery = config.network.discovery_port,
        control = config.network.control_port,
        audio = config.network.audio_port,
        budget_kbps = config.network.budget_kbps,
        "listening"
    );
    let layout_path = config.widgets.layout_path(&paths.config_file);
    let layout = Layout::load_or_create(&layout_path)?;
    let events = state.subscribe_events();
    let features = runtime.block_on(app::Features::start(
        &state,
        &config,
        layout,
        app::Backends {
            exec: companion_host::actions::system_executor(),
            capture: std::sync::Arc::new(companion_host::audio::capture::LoopbackSource {
                device: Some(config.audio.speaker_device.trim().to_string()).filter(|d| !d.is_empty()),
            }),
        },
    ));
    runtime.spawn(app::run(events, features));

    let server_state = state.clone();
    let server = runtime.spawn(async move {
        if let Err(e) = server::run(server_state, listeners).await {
            error!("network server stopped: {e}");
        }
    });

    if args.pair {
        let pin = state.open_pairing();
        info!("pairing PIN: {pin} (enter it on the PSP within 2 minutes)");
    }

    let result = if args.headless || !cfg!(feature = "tray") {
        if !args.headless {
            info!("built without the tray icon; running headless (stop with Ctrl+C)");
        }
        runtime.block_on(async {
            tokio::select! {
                r = tokio::signal::ctrl_c() => r.map_err(|e| format!("cannot wait for Ctrl+C: {e}")),
                _ = server => Err("network server stopped".to_string()),
            }
        })
    } else {
        #[cfg(feature = "tray")]
        {
            tray::run(state.clone(), paths.log_dir.clone(), paths.config_file.clone())
        }
        #[cfg(not(feature = "tray"))]
        unreachable!("headless is forced without the tray feature")
    };

    state.close_all(ByeReason::ShuttingDown);
    // Lets session writers send their Bye before the runtime drops the sockets.
    runtime.block_on(tokio::time::sleep(Duration::from_millis(300)));
    runtime.shutdown_timeout(Duration::from_secs(2));
    info!("stopped");
    result
}
