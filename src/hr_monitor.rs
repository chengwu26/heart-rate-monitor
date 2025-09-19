use std::error::Error;

use bluest::btuuid::{bluetooth_uuid_from_u16, characteristics::HEART_RATE_MEASUREMENT};
use bluest::{Adapter, Characteristic, Device, Uuid};
use tokio_stream::{Stream, StreamExt, adapters::Map};

pub const HRS_UUID: Uuid = bluetooth_uuid_from_u16(0x180D);

/// Maintain an HRS Bluetooth device
pub struct HeartRateMonitor {
    pub adapter: Adapter,
    pub device: Device,
    characteristic: Characteristic,
}

impl HeartRateMonitor {
    /// This function will use `adapter` to connect to the `device` once, without manually calling `HeartRateMonitor::connect`.
    pub async fn new(adapter: Adapter, device: Device) -> Result<Self, Box<dyn Error>> {
        if !device.is_connected().await {
            adapter.connect_device(&device).await?;
        }

        let hrm = HeartRateMonitor::get_hrm_characteristic(&device).await?;
        Ok(Self {
            adapter,
            device,
            characteristic: hrm,
        })
    }

    /// Enable notification of heart rate changes.
    ///
    /// Return a stream of heart rate value.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// let monitor = HeartRateMonitor::new(adapter, device);
    /// let stream = monitor.notify().await?;
    /// while let Some(Some(heart_rate)) = stream.next().await {
    ///     println!("Heart Rate: {heart_rate}");
    /// }
    /// ```
    pub async fn notify(
        &self,
    ) -> Result<
        Map<
            impl Stream<Item = Result<Vec<u8>, bluest::Error>> + '_,
            impl FnMut(Result<Vec<u8>, bluest::Error>) -> Option<u16>,
        >,
        Box<dyn Error>,
    > {
        self.characteristic
            .notify()
            .await
            .map(|s| s.map(|raw| HeartRateMonitor::parse(raw.ok()?)))
            .map_err(|e| Box::new(e) as Box<dyn Error>)
    }

    /// The connection status for HRS device.
    pub async fn is_connected(&self) -> bool {
        self.device.is_connected().await
    }

    /// Connect to the HRS device.
    ///
    /// This function is often used to reconnect to the device.
    pub async fn connect(&mut self) -> Result<(), Box<dyn Error>> {
        if self.device.is_connected().await {
            return Ok(());
        }

        self.adapter.connect_device(&self.device).await?;
        // Refresh Characteristic
        self.characteristic = HeartRateMonitor::get_hrm_characteristic(&self.device).await?;
        Ok(())
    }
}

impl HeartRateMonitor {
    /// Parse the raw data of heart rate from `device`.
    ///
    /// If the `data` is invalid, return None.
    fn parse(data: Vec<u8>) -> Option<u16> {
        let flag = *data.get(0)?;
        // Heart Rate Value Format
        let mut heart_rate_value = *data.get(1)? as u16;
        if flag & 0b00001 != 0 {
            heart_rate_value |= (*data.get(2)? as u16) << 8;
        }
        Some(heart_rate_value)
    }

    async fn get_hrm_characteristic(device: &Device) -> Result<Characteristic, Box<dyn Error>> {
        let heart_rate_service = device
            .discover_services_with_uuid(HRS_UUID)
            .await?
            .into_iter()
            .next()
            .ok_or("Device should has one heart rate service at least")?;

        let characteristic = heart_rate_service
            .discover_characteristics_with_uuid(HEART_RATE_MEASUREMENT)
            .await?
            .into_iter()
            .next()
            .ok_or(
                "HeartRateService should has one heart rate measurement characteristic at least",
            )?;

        Ok(characteristic)
    }
}
