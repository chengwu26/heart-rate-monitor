mod hr_monitor;
mod tasks;
mod utils;

use std::cell::{LazyCell, RefCell};
use std::io::Write;
use std::sync::Arc;

use anyhow::{Context, Result, bail, ensure};
use bluest::Adapter;
use camino::Utf8PathBuf;
use clap::Parser;
use tokio_stream::StreamExt;

use hr_monitor::{HRS_UUID, HeartRateMonitor};

const THEME_HOME: LazyCell<Utf8PathBuf> =
    LazyCell::new(|| utils::find_file("themes").expect("Theme library not found"));

#[derive(Parser)]
#[command(version)]
struct Cli {
    /// Custom port(0: system assignment)
    #[arg(short, long, value_name = "PORT", default_value_t = 3030)]
    port: u16,
    /// Run with <THEME> and set as default theme
    #[arg(short, long, value_name = "THEME")]
    theme: Option<String>,
    /// List all themes
    #[arg(short, long)]
    list: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    if cli.list {
        list_themes()?.for_each(|theme| println!("{theme}"));
        return Ok(());
    }
    if let Some(ref theme) = cli.theme {
        set_theme(theme)?
    }

    // Initial Bluetooth
    let adapter = Adapter::default()
        .await
        .context("Bluetooth adapter not found")?;
    adapter.wait_available().await?;

    let hrs_device = select_device(&adapter).await?;
    println!("Connecting");
    let monitor = Arc::new(RefCell::new(
        HeartRateMonitor::new(adapter, hrs_device).await?,
    ));

    // Start HTTP service
    tokio::spawn(async move {
        if let Err(e) = tasks::http_service(cli.port, THEME_HOME.join("default")).await {
            eprintln!("HTTP server stopped: {e:?}");
            std::process::exit(1);
        }
    });

    // The connection may be disconnected for various reasons.
    // In this case, try to reconnect it.
    let mut connect_retry = 0;
    while connect_retry < 5 {
        // If the reconnect is successful, reset the `connect_retry`.
        if monitor.borrow().is_connected().await {
            connect_retry = 0;
            println!("Connected");
        }

        // Block current task, if the device not disconnected
        tokio::select! {
            v = tasks::monitor_heart_rate(monitor.clone()) => { v? }
            _ = tasks::check_connection_status(monitor.clone()) => {}
        }

        connect_retry += 1;
        eprintln!("Bluetooth Device disconnected, try to reconnect({connect_retry}).");
        if let Err(e) = monitor.borrow_mut().connect().await {
            eprintln!("Failed to reconnect: {e}")
        }
    }
    println!("Program Exit");
    Ok(())
}

/// Scan HRS devices, and return a device which user selected
// TODO: This implementation is very stupid, improve it.
async fn select_device(adapter: &Adapter) -> Result<bluest::Device> {
    use std::time::{Duration, Instant};

    let mut devices = Vec::new();
    let mut devices_stream = adapter
        .discover_devices(&[HRS_UUID])
        .await
        .context("Failed to scan/discover device")?;
    println!("Scanning HRS devices...");
    loop {
        let start = Instant::now();
        while devices.is_empty()
            || Instant::now().duration_since(start) < Duration::from_millis(500)
        {
            if let Some(Ok(device)) = devices_stream.next().await
                && !devices.contains(&device)
            {
                devices.push(device);
            }
        }

        // Print devices
        for (i, device) in devices.iter().enumerate() {
            println!("{}) {device}", i + 1);
        }

        // Let user select a device
        let mut input = String::new();
        loop {
            print!("\nInput a device index (q: quit, 0: refresh): ");
            std::io::stdout().flush().unwrap();

            std::io::stdin()
                .read_line(&mut input)
                .context("Failed to read from stdin.")?;

            if let Ok(index) = input.trim().parse::<usize>()
                && index > 0
                && index <= devices.len()
            {
                let device = devices.into_iter().nth(index - 1).unwrap();
                println!("Selected Device: [{}] {}", device, device.id());
                return Ok(device);
            } else if input.trim() == "q" {
                std::process::exit(0);
            } else if input.trim() == "0" {
                break;
            }
            input.clear();
        }
    }
}

fn set_theme(theme: &str) -> Result<()> {
    ensure!(
        !theme.contains(std::path::MAIN_SEPARATOR),
        "Invalid theme name"
    );

    let theme_file = THEME_HOME.join(format!("{theme}.html"));
    ensure!(
        theme_file.is_file(),
        "Failed to set theme: no such theme in your theme library '{}'",
        *THEME_HOME
    );

    let link_file = THEME_HOME.join("default");
    if link_file.exists() {
        std::fs::remove_file(&link_file)
            .context("Failed to set theme: Can't to remove old symlink")?;
    }
    #[cfg(target_os = "windows")]
    if let Err(e) = std::os::windows::fs::symlink_file(theme_file, link_file) {
        use std::io::ErrorKind;
        if let ErrorKind::PermissionDenied = e.kind() {
            bail!(
                "Failed to set theme: You can try enabling Developer Mode or running the process as an administrator."
            )
        }
        bail!("Failed to set theme: Can't create symlink")
    }
    #[cfg(not(target_os = "windows"))]
    std::os::unix::fs::symlink(theme_file, link_file)
        .context("Failed to set theme: Can't create symlink")?;
    Ok(())
}

fn list_themes() -> Result<impl Iterator<Item = String>> {
    Ok(THEME_HOME.read_dir_utf8()?.filter_map(|e| {
        let entry = e.ok()?;
        let name = entry.file_name();
        if name.ends_with(".html") {
            Some(name.trim_end_matches(".html").to_string())
        } else {
            None
        }
    }))
}
