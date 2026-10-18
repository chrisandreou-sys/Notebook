mod ai;
#[cfg(test)]
mod integration;
mod media;
mod model;
mod retrieval;
mod studio;
mod style;
mod web;

use std::{env, net::SocketAddr, path::PathBuf, sync::{Arc, Mutex}};

use anyhow::{bail, Context, Result};
use model::Store;

#[tokio::main]
async fn main() -> Result<()> {
    let mut host = "127.0.0.1".to_owned();
    let mut port: u16 = 3040;
    let mut data_dir = env::var_os("NOTEBOOK_DATA_DIR").map(PathBuf::from);
    let mut no_open = false;
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--host" => host = args.next().context("--host needs an address")?,
            "--port" => port = args.next().context("--port needs a number")?.parse().context("invalid port")?,
            "--data-dir" => data_dir = Some(PathBuf::from(args.next().context("--data-dir needs a path")?)),
            "--no-open" => no_open = true,
            "--help" | "-h" => {
                println!("Notebook RS {}\n\nUsage: notebook-rs [--host 127.0.0.1] [--port 3040] [--data-dir PATH] [--no-open]\n\nBy default only this computer can connect. Set NOTEBOOK_DATA_DIR to override storage.", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            "--version" | "-V" => {
                println!("notebook-rs {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            _ => bail!("unknown argument: {arg} (try --help)"),
        }
    }

    let data_dir = data_dir.unwrap_or_else(model::default_data_dir);
    let store = Store::open(data_dir).context("opening notebook data")?;
    let state = Arc::new(web::AppState {
        store: Mutex::new(store),
        client: reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(180))
            .build()?,
        csrf: uuid::Uuid::new_v4().to_string(),
    });
    let listener = tokio::net::TcpListener::bind(format!("{host}:{port}"))
        .await
        .with_context(|| format!("binding {host}:{port}"))?;
    let addr: SocketAddr = listener.local_addr()?;
    let url = format!("http://127.0.0.1:{}", addr.port());
    println!("Notebook RS {} — {url}", env!("CARGO_PKG_VERSION"));
    println!("Data: {}", state.store.lock().unwrap().root.display());
    if host != "127.0.0.1" && host != "localhost" && host != "::1" {
        eprintln!("Warning: listening beyond localhost. Notebook data has no user accounts; use a trusted network or a reverse proxy with authentication.");
    }
    if !no_open {
        let _ = open_browser(&url);
    }
    axum::serve(listener, web::router(state)).await?;
    Ok(())
}

// Only the OS's existing browser launcher is invoked; no web assets or scripts are downloaded.
fn open_browser(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    { std::process::Command::new("cmd").args(["/C", "start", "", url]).spawn()?; }
    #[cfg(target_os = "macos")]
    { std::process::Command::new("open").arg(url).spawn()?; }
    #[cfg(all(unix, not(target_os = "macos")))]
    { std::process::Command::new("xdg-open").arg(url).spawn()?; }
    Ok(())
}
