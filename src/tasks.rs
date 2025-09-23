//! Asynchronous task used in 'main.rs'

use std::cell::RefCell;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::ops::Deref;
use std::path::Path;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, anyhow};
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
///
/// Note: This task will hold `monitor`'s shared reference across an await suspension point
pub async fn check_connection_status<T>(monitor: T)
where
    T: Deref<Target = RefCell<HeartRateMonitor>>,
{
    while monitor.borrow().is_connected().await {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await
    }
}

/// Get the heart rate from `monitor`
///
/// Note: This task will hold `monitor`'s shared reference until it's completed
pub async fn monitor_heart_rate<T>(monitor: T) -> Result<()>
where
    T: Deref<Target = RefCell<HeartRateMonitor>>,
{
    let monitor = monitor.borrow();
    let mut heart_rate_stream = monitor.notify().await?;

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
    Err(anyhow!(
        "Device may be disconnected or the device has sent invalid data"
    ))
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
pub async fn http_service(port: u16, html_file: impl AsRef<Path>) -> Result<()> {
    let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 1), port))
        .await
        .with_context(|| format!("Failed to listening at the port: {}", port))?;
    let port = listener.local_addr().unwrap().port();
    println!("Listening at http://127.0.0.1:{port}");

    let html = tokio::fs::read_to_string(&html_file)
        .await
        .with_context(|| format!("Can't open the file: {}", html_file.as_ref().display()))?
        .replace("{{PORT}}", &format!("{port}"));
    let html = Arc::new(html);

    loop {
        let (stream, addr) = listener.accept().await.context("Failed to get client")?;
        tokio::spawn(process_connection(stream, addr, html.clone()));
    }
}

async fn process_connection(
    mut stream: TcpStream,
    addr: SocketAddr,
    html: impl Deref<Target = String>,
) {
    let mut buffer = [0; 1024];
    if let Err(e) = stream.read(&mut buffer).await {
        eprintln!("Failed to read scoket from {addr}: {e}");
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
        eprintln!("Failed to response the HTTP request from {addr}: {e}");
        return;
    }
    if let Err(e) = stream.flush().await {
        eprintln!("Failed to response the HTTP request from {addr}: {e}");
    }
}
