mod hr_monitor;
mod tasks;

use std::cell::RefCell;
use std::error::Error;
use std::io::Write;
use std::sync::Arc;

use bluest::Adapter;
use clap::Parser;
use tokio_stream::StreamExt;

use hr_monitor::{HRS_UUID, HeartRateMonitor};

#[derive(Parser)]
struct Cli {
    /// Optional custom port(0: system assignment)
    #[arg(short, long, value_name = "PORT", default_value_t = 3030)]
    port: u16,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let cli = Cli::parse();

    // Initial Bluetooth
    let adapter = Adapter::default()
        .await
        .ok_or("Bluetooth adapter not found")?;
    adapter.wait_available().await?;

    let hrs_device = select_device(&adapter).await?;
    println!("Connecting");
    let monitor = Arc::new(RefCell::new(
        HeartRateMonitor::new(adapter, hrs_device).await?,
    ));

    // Start HTTP service
    tokio::spawn(tasks::http_service(cli.port));

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
            _ = tasks::monitor_heart_rate(monitor.clone()) => {}
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
async fn select_device(adapter: &Adapter) -> Result<bluest::Device, bluest::Error> {
    use std::time::{Duration, Instant};

    let mut devices = Vec::new();
    let mut devices_stream = adapter.discover_devices(&[HRS_UUID]).await?;
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
                .expect("Failed to read from stdin.");

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
