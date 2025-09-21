//! Asynchronous task used in 'main.rs'

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddrV4};
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;
use tokio_stream::StreamExt;

use crate::hr_monitor::HeartRateMonitor;

#[derive(Debug)]
struct HeartRate {
    ts_millis: u128,
    value: u16,
}

static HEART_RATE: OnceLock<RwLock<HeartRate>> = OnceLock::new();

/// Check connection status every 2 seconds and complete when the connection is broken.
pub async fn check_connection_status<T>(monitor: T)
where
    T: Deref<Target = RefCell<HeartRateMonitor>>,
{
    while monitor.borrow().is_connected().await {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await
    }
}

/// Get the heart rate from `monitor` and synchronize to task `http_service`
pub async fn monitor_heart_rate<T>(monitor: T)
where
    T: Deref<Target = RefCell<HeartRateMonitor>>,
{
    let monitor = monitor.borrow();
    let mut heart_rate_stream = match monitor.notify().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to enable notification for the HRS device: {e}");
            std::process::exit(1);
        }
    };

    while let Some(Some(heart_rate)) = heart_rate_stream.next().await {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("Invalid System Time")
            .as_millis();

        match HEART_RATE.get() {
            None => HEART_RATE
                .set(RwLock::from(HeartRate {
                    ts_millis: timestamp,
                    value: heart_rate,
                }))
                .unwrap(),
            Some(rate) => {
                let mut rate = rate.write().await;
                rate.ts_millis = timestamp;
                rate.value = heart_rate;
            }
        }
    }
}

/// Starts a tiny HTTP service that provides a heart rate data query interface listening on a specified port.
/// Clients can obtain the current heart rate value by sending a GET request to the '/heart-rate'
/// endpoint. The service will return an HTTP response containing real-time heart rate data and
/// Unix timestamp with millisecond.
///
/// Example Request:
/// GET http://localhost:3030/heart-rate
///
/// The response data is in JSON format.
pub async fn http_service<P: AsRef<Path>>(port: u16, html_file: P) {
    let listener =
        match TcpListener::bind(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port)).await {
            Ok(t) => {
                println!(
                    "Listening at http://127.0.0.1:{}",
                    t.local_addr().unwrap().port()
                );
                t
            }
            Err(e) => {
                eprintln!("Failed to use port 3030: {e}\nHTTP Service Exit");
                return;
            }
        };

    // Concatenate path
    // - Release: paths relative to executable directory
    // - Debug: paths relative to working directory (for testing)
    let html_file = if html_file.as_ref().is_relative() && cfg!(not(debug_assertions)) {
        let abs_path = std::env::current_exe()
            .expect("Failed to get the path to the current executable")
            .parent()
            .expect("Executable has no parent directory")
            .to_path_buf();
        abs_path.join(html_file)
    } else {
        html_file.as_ref().to_owned()
    };

    let html = tokio::fs::read_to_string(&html_file)
        .await
        .unwrap_or_else(|_| {
            eprintln!("The HTML file '{}' not found", html_file.display());
            String::from("HTML file not found")
        })
        .replace(
            "{{PORT}}",
            &format!("{}", listener.local_addr().unwrap().port()),
        );
    let html = Arc::new(html);

    loop {
        let stream = match listener.accept().await {
            Ok((s, _)) => s,
            Err(e) => {
                eprintln!("Failed to get client: {e}");
                break;
            }
        };
        tokio::spawn(process_connection(stream, html.clone()));
    }
}

async fn process_connection<T>(mut stream: TcpStream, html: T)
where
    T: Deref<Target = String>,
{
    let mut buffer = [0; 1024];
    if let Err(e) = stream.read(&mut buffer).await {
        eprintln!("{e}");
        return;
    }

    let get = b"GET / HTTP";
    let heart_rate = b"GET /heart-rate HTTP";

    let (status_line, contents) = if buffer.starts_with(get) {
        ("HTTP/1.1 200 OK", (*html).clone())
    } else if buffer.starts_with(heart_rate) {
        let response = if let Some(rate) = HEART_RATE.get() {
            let rate = rate.read().await;
            format!(
                "{{\"ts_millis\": {}, \"rate\": {}}}",
                rate.ts_millis, rate.value
            )
        } else {
            String::from("{\"ts\": none, \"rate\": none}")
        };

        ("HTTP/1.1 200 OK", response)
    } else {
        ("HTTP/1.1 404 NOT FOUND", String::from("404 Not Found"))
    };

    let response = format!(
        "{}\r\nContent-Type: {}\r\nContent-Length: {}\r\n\r\n{}",
        status_line,
        if buffer.starts_with(heart_rate) {
            "application/json"
        } else {
            "text/html"
        },
        contents.len(),
        contents
    );

    if let Err(e) = stream.write(response.as_bytes()).await {
        eprintln!("{e}");
        return;
    }
    if let Err(e) = stream.flush().await {
        eprintln!("{e}");
        return;
    }
}
