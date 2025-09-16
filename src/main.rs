use std::cell::RefCell;
use std::error::Error;
use std::ops::Deref;
use std::sync::Arc;
use std::sync::atomic::{AtomicU16, Ordering};

use bluest::btuuid::{bluetooth_uuid_from_u16, characteristics::HEART_RATE_MEASUREMENT};
use bluest::{Adapter, Characteristic, Device, Uuid};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_stream::{Stream, StreamExt};

const HRS_UUID: Uuid = bluetooth_uuid_from_u16(0x180D);
static LATEST_RATE: AtomicU16 = AtomicU16::new(0);

struct HeartRateMonitor {
    adapter: Adapter,
    device: Device,
    characteristic: Characteristic,
}

impl HeartRateMonitor {
    async fn new(adapter: Adapter, device: Device) -> Result<Self, Box<dyn Error>> {
        // Connect
        if !device.is_connected().await {
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
            adapter,
            device,
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

    async fn reconnect(&mut self) -> Result<(), Box<dyn Error>> {
        self.adapter.connect_device(&self.device).await?;

        #[cfg(target_os = "windows")]
        {
            let heart_rate_service = self
                .device
                .discover_services_with_uuid(HRS_UUID)
                .await?
                .into_iter()
                .next()
                .ok_or("Device should has one heart rate service at least")?;

            self.characteristic = heart_rate_service
            .discover_characteristics_with_uuid(HEART_RATE_MEASUREMENT)
            .await?
            .into_iter()
            .next()
            .ok_or(
                "HeartRateService should has one heart rate measurement characteristic at least",
            )?;
        }
        Ok(())
    }

    fn parse(raw_data: Vec<u8>) -> Result<u16, Box<dyn Error>> {
        let flag = *raw_data.get(0).ok_or("No flag")?;
        // Heart Rate Value Format
        let mut heart_rate_value = *raw_data.get(1).ok_or("No heart rate u8")? as u16;
        if flag & 0b00001 != 0 {
            heart_rate_value |= (*raw_data.get(2).ok_or("No heart rate u16")? as u16) << 8;
        }
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

    let hrs_device = select_device(&adapter).await?;

    // Start HTTP service
    tokio::spawn(http_service());

    let monitor = Arc::new(RefCell::new(
        HeartRateMonitor::new(adapter, hrs_device).await?,
    ));

    // The connection may be disconnected for various reasons.
    // In this case, try to reconnect it.
    let mut connect_retry = 0;
    while connect_retry < 5 {
        if monitor.borrow().is_connected().await {
            connect_retry = 0;
            println!("Connected");
        }

        let monitor_cloned = monitor.clone();
        tokio::select! {
            _ = hr_monitor(monitor.clone()) => {}
            _ = async move {
                    while monitor_cloned.borrow().is_connected().await {
                        tokio::time::sleep(std::time::Duration::from_secs(1)).await
                } } => {}
        }
        connect_retry += 1;
        eprintln!("Bluetooth Device disconnected, try to reconnect({connect_retry}).");
        if let Err(e) = monitor.borrow_mut().reconnect().await {
            eprintln!("Failed to reconnect: {e}")
        }
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    }
    Ok(())
}

async fn select_device(adapter: &Adapter) -> Result<bluest::Device, bluest::Error> {
    use std::time::{Duration, Instant};

    let mut devices_stream = adapter.discover_devices(&[HRS_UUID]).await?;
    let mut devices = Vec::new();

    println!("Scanning HRS devices...");
    let start = Instant::now();
    while Instant::now().duration_since(start) < Duration::from_millis(1500) || devices.is_empty() {
        if let Some(Ok(device)) = devices_stream.next().await
            && !devices.contains(&device)
        {
            devices.push(device);
        }
    }
    println!("Done");

    // Print devices
    for (i, device) in devices.iter().enumerate() {
        println!("{}) {device}", i + 1);
    }

    // Let user select a device
    let mut input = String::new();
    loop {
        print!("Input a device index (q: quit, 0: refresh): ");
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
        } else if input.as_str() == "q\n" {
            std::process::exit(0);
        } else if input.as_str() == "0\n" {
            unimplemented!("Unimplement refresh");
        }
        input.clear();
    }
}

async fn http_service() {
    let listener = match TcpListener::bind("127.0.0.1:3030").await {
        Ok(t) => {
            println!("Listening at http://127.0.0.1:3030");
            t
        }
        Err(e) => {
            eprintln!("Failed to use port 3030: {e}\nHTTP Service Exit");
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
        let rate = LATEST_RATE.load(Ordering::SeqCst);

        let response = if rate == 0 {
            String::from("{\"rate\": none}")
        } else {
            format!("{{\"rate\": {rate}}}")
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

async fn hr_monitor<T: Deref<Target = RefCell<HeartRateMonitor>>>(monitor: T) {
    let monitor = monitor.borrow();
    let mut heart_rate_stream = match monitor.notify().await {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Failed to enable notification for the HRS device: {e}");
            std::process::exit(1);
        }
    };

    while let Some(Ok(raw_data)) = heart_rate_stream.next().await {
        let heart_rate_value =
            HeartRateMonitor::parse(raw_data).expect("The HRS device sent invalid data.");
        LATEST_RATE.store(heart_rate_value, Ordering::SeqCst);
    }
}
