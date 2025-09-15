use std::error::Error;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bluest::btuuid::{bluetooth_uuid_from_u16, characteristics::HEART_RATE_MEASUREMENT};
use bluest::{Adapter, Characteristic, Device, Uuid};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
// use tokio::signal;
use tokio_stream::{Stream, StreamExt};

const HRS_UUID: Uuid = bluetooth_uuid_from_u16(0x180D);
static LATEST_RATE: AtomicU16 = AtomicU16::new(0);

struct HeartRateMonitor {
    device: Device,
    // service: Service,
    characteristic: Characteristic,
}

impl HeartRateMonitor {
    async fn new(adapter: &Adapter, device: Device) -> Result<Self, Box<dyn Error>> {
        // Connect
        if !device.is_connected().await {
            println!("Connecting device");
            adapter.connect_device(&device).await?;
            println!("Connected");
        }

        // Discover services
        let heart_rate_service = device
            .discover_services_with_uuid(HRS_UUID)
            .await?
            .into_iter()
            .next()
            .ok_or("Device should has one heart rate service at least")?;

        let heart_rate_measurement = heart_rate_service
            .discover_characteristics_with_uuid(HEART_RATE_MEASUREMENT)
            .await?
            .into_iter()
            .next()
            .ok_or(
                "HeartRateService should has one heart rate measurement characteristic at least",
            )?;

        Ok(Self {
            device,
            // service: heart_rate_service,
            characteristic: heart_rate_measurement,
        })
    }

    async fn notify(
        &self,
    ) -> Result<impl Stream<Item = Result<Vec<u8>, bluest::Error>> + '_, Box<dyn Error>> {
        self.characteristic
            .notify()
            .await
            .map_err(|e| Box::new(e) as Box<dyn Error>)
    }

    async fn is_connected(&self) -> bool {
        self.device.is_connected().await
    }

    fn parse(raw_data: Vec<u8>) -> Result<u16, Box<dyn Error>> {
        let flag = *raw_data.get(0).ok_or("No flag")?;

        // Heart Rate Value Format
        let mut heart_rate_value = *raw_data.get(1).ok_or("No heart rate u8")? as u16;
        if flag & 0b00001 != 0 {
            heart_rate_value |= (*raw_data.get(2).ok_or("No heart rate u16")? as u16) << 8;
        }

        // Sensor Contact Supported
        // let mut sensor_contact = None;
        // if flag & 0b00100 != 0 {
        //     sensor_contact = Some(flag & 0b00010 != 0)
        // }
        Ok(heart_rate_value)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    // Initial Bluetooth
    let adapter = Adapter::default()
        .await
        .ok_or("Bluetooth adapter not found")?;
    adapter.wait_available().await?;

    // Get the HRS device
    let hrs_device = {
        let devices = adapter.connected_devices_with_services(&[HRS_UUID]).await?;
        // TODO: 多个设备让用户选择
        if let Some(device) = devices.into_iter().next() {
            device
        } else {
            println!("Scan HRS device...");
            adapter
                .discover_devices(&[HRS_UUID])
                .await?
                .next()
                .await
                .ok_or("Failed to discover device")??
        }
    };

    let device_name = hrs_device
        .name_async()
        .await
        .unwrap_or_else(|_| String::from("unknow device"));
    println!("Selected Device: [{}] {:?}", hrs_device, device_name);

    // Task: http service
    tokio::spawn(async move {
        let listener = match TcpListener::bind("127.0.0.1:3030").await {
            Ok(t) => {
                println!("Listening at http://127.0.0.1:3030");
                t
            }
            Err(e) => {
                eprintln!("Failed to use port 3030: {e}");
                eprintln!("HTTP Service Exit");
                return;
            }
        };

        loop {
            let stream = match listener.accept().await {
                Ok((s, _)) => s,
                Err(e) => {
                    eprintln!("Failed to get client: {e}");
                    break;
                }
            };
            tokio::spawn(process_connection(stream));
        }
        eprintln!("HTTP Service Exit");
    });

    // Task: record heart rate
    let (heart_rate_tx, mut heart_rate_rx) = tokio::sync::mpsc::channel(10);
    tokio::spawn(async move {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("The system time is incorrect")
            .as_millis();
        let file_name = format!("{device_name}_{timestamp}.txt",);
        let mut file = match OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file_name)
            .await
        {
            Ok(f) => f,
            Err(e) => {
                eprintln!("Failed to create/open file '{file_name}': {e}");
                return;
            }
        };

        while let Some((timestamp, heart_rate)) = heart_rate_rx.recv().await {
            if let Err(e) = file
                .write_all(format!("{timestamp}\t{heart_rate}\n").as_bytes())
                .await
            {
                eprintln!("Filed write file '{file_name}': {e}");
                break;
            }
        }
        eprintln!("Record Task Exit.");
    });

    let monitor = Arc::new(HeartRateMonitor::new(&adapter, hrs_device).await?);

    // Task: read heart rate from HRS device
    let monitor_cloned = monitor.clone();
    tokio::spawn(async move {
        let mut heart_rate_stream = match monitor_cloned.notify().await {
            Ok(s) => s,
            Err(e) => {
                eprintln!("Failed to enable notification for the HRS device: {e}");
                std::process::exit(1);
            }
        };

        while let Some(Ok(raw_data)) = heart_rate_stream.next().await {
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("The system time is incorrect")
                .as_millis();

            let heart_rate_value =
                HeartRateMonitor::parse(raw_data).expect("The HRS device sent invalid data.");
            // println!("Timestamp: {timestamp}, HeartRate: {heart_rate_value}");
            LATEST_RATE.store(heart_rate_value, Ordering::SeqCst);
            let _ = heart_rate_tx.send((timestamp, heart_rate_value)).await;
        }
    });

    // tokio::select! {
    // _ = signal::ctrl_c() => {},
    // _ = async move {
    while monitor.is_connected().await {}
    // } => {},
    // };
    println!("Device disconnected.");
    Ok(())
}

async fn process_connection(mut stream: TcpStream) {
    let mut buffer = [0; 1024];
    if let Err(e) = stream.read(&mut buffer).await {
        eprintln!("{e}");
        return;
    }

    let get = b"GET / HTTP";
    let heart_rate = b"GET /heart-rate HTTP";

    let (status_line, contents) = if buffer.starts_with(get) {
        let html = tokio::fs::read_to_string("heart_rate.html")
            .await
            .unwrap_or_else(|_| String::from("HTML file not found"));
        ("HTTP/1.1 200 OK", html)
    } else if buffer.starts_with(heart_rate) {
        let response = format!("{{\"rate\": {}}}", LATEST_RATE.load(Ordering::SeqCst));
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
