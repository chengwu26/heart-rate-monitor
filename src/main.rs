use std::error::Error;
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use bluest::btuuid::{bluetooth_uuid_from_u16, characteristics::HEART_RATE_MEASUREMENT};
use bluest::{Adapter, Characteristic, Device, Uuid};
use futures_lite::{stream::StreamExt, Stream};
use tokio::fs::OpenOptions;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const HRS_UUID: Uuid = bluetooth_uuid_from_u16(0x180D);
static LATEST_RATE: AtomicU16 = AtomicU16::new(0);

struct HeartRateMonitor {
    // device: Device,
    // service: Service,
    characteristic: Characteristic,
}

impl HeartRateMonitor {
    async fn new(adapter: &Adapter, device: Device) -> Result<Self, Box<dyn Error>> {
        // Connect
        if !device.is_connected().await {
            println!("Connecting device");
            adapter.connect_device(&device).await?;
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
            // device,
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
        .unwrap_or(String::from("unknow"));
    println!("Selected Device: [{}] {:?}", hrs_device, device_name);

    let monitor = HeartRateMonitor::new(&adapter, hrs_device).await?;
    let mut heart_rate_stream = monitor.notify().await?;

    let (tx, mut rx) = tokio::sync::mpsc::channel(10);
    tokio::spawn(init_http_server());
    tokio::spawn(async move {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("The system time is incorrect")
            .as_millis();
        let file_name = format!("{device_name}_{timestamp}.txt",);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_name)
            .await
            .expect("Failed to open/create file.");

        while let Some((timestamp, heart_rate)) = rx.recv().await {
            file.write_all(format!("{timestamp}\t{heart_rate}\n").as_bytes())
                .await
                .unwrap()
        }
    });

    while let Some(Ok(raw_data)) = heart_rate_stream.next().await {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("The system time is incorrect")
            .as_millis();

        let heart_rate_value = parse_heart_rate_data(raw_data)?;
        // println!("Timestamp: {timestamp}, HeartRate: {heart_rate_value}");
        LATEST_RATE.store(heart_rate_value, Ordering::SeqCst);
        tx.send((timestamp, heart_rate_value)).await.unwrap();
    }
    Ok(())
}

fn parse_heart_rate_data(data: Vec<u8>) -> Result<u16, Box<dyn Error>> {
    let flag = *data.get(0).ok_or("No flag")?;

    // Heart Rate Value Format
    let mut heart_rate_value = *data.get(1).ok_or("No heart rate u8")? as u16;
    if flag & 0b00001 != 0 {
        heart_rate_value |= (*data.get(2).ok_or("No heart rate u16")? as u16) << 8;
    }

    // Sensor Contact Supported
    // let mut sensor_contact = None;
    // if flag & 0b00100 != 0 {
    //     sensor_contact = Some(flag & 0b00010 != 0)
    // }
    Ok(heart_rate_value)
}

async fn init_http_server() {
    let listener = TcpListener::bind("127.0.0.1:3030").await.unwrap();
    println!("Listening at http://127.0.0.1:3030");

    loop {
        let (stream, _) = listener.accept().await.unwrap();
        tokio::spawn(process_connection(stream));
    }
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
