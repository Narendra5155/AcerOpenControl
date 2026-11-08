//! CoolerControl `DeviceService` backed by aocd.
//!
//! - One device ("aoc") with a speed channel per fan (`fan1`, `fan2`, ...).
//! - `FixedDuty` / `EnableManualFanControl` borrow the fans from aocd (a lease
//!   tied to this plugin's connection); `ResetChannel` hands a fan to the BIOS,
//!   and once every fan is reset the lease is released so AcerOpenControl's own
//!   fan mode takes over again.
//! - CPU/GPU temperatures as aocd reads them (a sleeping dGPU is not woken).
//! - aocd keeps its fail-safes (emergency 100 %, minimum floor, BIOS fallback).

use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Mutex;
use tonic::{Request, Response, Status};

use crate::aocd::{Aocd, AocdError};
use crate::device_service::v1::device_service_server::DeviceService;
use crate::device_service::v1::{
    health_response, CustomFunctionOneRequest, CustomFunctionOneResponse, EnableManualFanControlRequest,
    EnableManualFanControlResponse, FixedDutyRequest, FixedDutyResponse, HealthRequest, HealthResponse,
    InitializeDeviceRequest, InitializeDeviceResponse, LcdRequest, LcdResponse, LightingRequest, LightingResponse,
    ListDevicesRequest, ListDevicesResponse, ResetChannelRequest, ResetChannelResponse, ShutdownRequest,
    ShutdownResponse, SpeedProfileRequest, SpeedProfileResponse, StatusRequest, StatusResponse,
};
use crate::models::v1::{self as m, channel_info, status::FanSpeed, status::Metric};
use crate::{SERVICE_ID, VERSION};

pub const DEVICE_ID: &str = "aoc";
const TEMP_CPU: &str = "temp1";
const TEMP_GPU: &str = "temp2";

fn to_status(e: AocdError) -> Status {
    match e {
        AocdError::Unavailable(m) => Status::unavailable(format!("aocd unavailable: {m}")),
        AocdError::Refused { message, forbidden: true } => Status::permission_denied(format!(
            "{message}. aocd only lends the fans to accounts listed in lease_users in \
             /etc/acer-open-control/config.json (default: cc-plugin-user)"
        )),
        AocdError::Refused { message, .. } => Status::failed_precondition(message),
    }
}

/// "fan2" -> 1
pub fn fan_index(channel_id: &str) -> Option<usize> {
    channel_id.strip_prefix("fan")?.parse::<usize>().ok()?.checked_sub(1)
}

pub struct AocService {
    pub aocd: Arc<Aocd>,
    started: Instant,
    fan_count: Mutex<Option<usize>>,
    aocd_socket: String,
}

impl AocService {
    pub fn new(aocd: Arc<Aocd>, aocd_socket: impl Into<String>) -> AocService {
        AocService { aocd, started: Instant::now(), fan_count: Mutex::new(None), aocd_socket: aocd_socket.into() }
    }

    async fn model(&self) -> Result<Value, Status> {
        self.aocd.call(json!({"cmd": "get_model"})).await.map_err(to_status)
    }

    async fn fans(&self) -> Result<usize, Status> {
        if let Some(n) = *self.fan_count.lock().await {
            return Ok(n);
        }
        let model = self.model().await?;
        let n = model["capabilities"]["fans"].as_u64().unwrap_or(0) as usize;
        *self.fan_count.lock().await = Some(n);
        Ok(n)
    }

    fn check_device(id: &str) -> Result<(), Status> {
        if id == DEVICE_ID {
            Ok(())
        } else {
            Err(Status::not_found(format!("unknown device {id:?}")))
        }
    }

    async fn check_channel(&self, device: &str, channel: &str) -> Result<(usize, usize), Status> {
        Self::check_device(device)?;
        let n = self.fans().await?;
        match fan_index(channel) {
            Some(i) if i < n => Ok((i, n)),
            _ => Err(Status::not_found(format!("unknown channel {channel:?}"))),
        }
    }

    /// Build the CoolerControl device description from aocd's model info.
    pub fn device_from_model(model: &Value, aocd_socket: &str) -> Option<m::Device> {
        let caps = &model["capabilities"];
        let labels: Vec<String> =
            caps["fan_labels"].as_array()?.iter().map(|l| l.as_str().unwrap_or("").to_string()).collect();
        if labels.is_empty() {
            return None;
        }
        let writable = model["read_only"] != json!(true);
        let channels = labels
            .iter()
            .enumerate()
            .map(|(i, label)| {
                let info = m::ChannelInfo {
                    label: Some(format!("{label} Fan")),
                    options: Some(channel_info::Options::SpeedOptions(m::SpeedOptions {
                        min_duty: 0,
                        max_duty: 100,
                        fixed_enabled: writable,
                        extension: None,
                    })),
                };
                (format!("fan{}", i + 1), info)
            })
            .collect();
        let temps = [
            (TEMP_CPU.to_string(), m::TempInfo { label: "CPU (aocd)".into(), number: 1 }),
            (TEMP_GPU.to_string(), m::TempInfo { label: "GPU (aocd)".into(), number: 2 }),
        ]
        .into_iter()
        .collect();
        let name = model["model"]["name"].as_str().unwrap_or("AcerOpenControl").to_string();
        let product = model["machine"]["product_name"].as_str().unwrap_or("").to_string();
        Some(m::Device {
            id: DEVICE_ID.into(),
            name: "AcerOpenControl".into(),
            uid_info: (!product.is_empty()).then(|| format!("aoc:{product}")),
            info: Some(m::DeviceInfo {
                channels,
                temps,
                lighting_speeds: vec![],
                temp_min: Some(20.0),
                temp_max: Some(100.0),
                profile_min_length: None,
                profile_max_length: None,
                model: Some(name),
                driver_info: Some(m::DriverInfo {
                    name: Some("AcerOpenControl (aocd)".into()),
                    version: model["version"].as_str().map(str::to_string).or(Some(VERSION.into())),
                    locations: vec![aocd_socket.to_string()],
                }),
            }),
        })
    }

    /// Translate an aocd status reply into CoolerControl channel statuses.
    pub fn statuses(st: &Value) -> Vec<m::Status> {
        let mut out = Vec::new();
        if let Some(t) = st["cpu"].as_f64() {
            out.push(m::Status { id: TEMP_CPU.into(), metric: Some(Metric::Temp(t)) });
        }
        if let Some(t) = st["gpu"].as_f64() {
            out.push(m::Status { id: TEMP_GPU.into(), metric: Some(Metric::Temp(t)) });
        }
        for (i, f) in st["fans"].as_array().into_iter().flatten().enumerate() {
            out.push(m::Status {
                id: format!("fan{}", i + 1),
                metric: Some(Metric::Speed(FanSpeed {
                    duty: f["duty"].as_f64(),
                    rpm: f["rpm"].as_u64().map(|r| r as u32),
                })),
            });
        }
        out
    }
}

#[tonic::async_trait]
impl DeviceService for AocService {
    async fn health(&self, _r: Request<HealthRequest>) -> Result<Response<HealthResponse>, Status> {
        let status = match self.aocd.call(json!({"cmd": "status"})).await {
            Ok(st) if st["read_only"] == json!(true) => health_response::Status::Warning,
            Ok(st) if !st["fault"].is_null() => health_response::Status::Warning,
            Ok(_) => health_response::Status::Ok,
            Err(e) => {
                log::warn!("{e}");
                health_response::Status::Error
            }
        };
        Ok(Response::new(HealthResponse {
            name: SERVICE_ID.into(),
            version: VERSION.into(),
            status: status.into(),
            uptime_seconds: self.started.elapsed().as_secs(),
        }))
    }

    async fn list_devices(&self, _r: Request<ListDevicesRequest>) -> Result<Response<ListDevicesResponse>, Status> {
        let model = self.model().await?;
        let devices: Vec<m::Device> = Self::device_from_model(&model, &self.aocd_socket).into_iter().collect();
        *self.fan_count.lock().await = Some(model["capabilities"]["fans"].as_u64().unwrap_or(0) as usize);
        log::info!("exposing {} device(s) to CoolerControl", devices.len());
        Ok(Response::new(ListDevicesResponse { devices }))
    }

    async fn initialize_device(
        &self,
        r: Request<InitializeDeviceRequest>,
    ) -> Result<Response<InitializeDeviceResponse>, Status> {
        Self::check_device(&r.get_ref().device_id)?;
        // Also called after resume. aocd reapplies its own state; just make sure
        // our connection (and with it the lease) is alive.
        self.aocd.call(json!({"cmd": "status"})).await.map_err(to_status)?;
        Ok(Response::new(InitializeDeviceResponse {}))
    }

    async fn shutdown(&self, _r: Request<ShutdownRequest>) -> Result<Response<ShutdownResponse>, Status> {
        if let Err(e) = self.aocd.release().await {
            log::warn!("releasing fans on shutdown: {e}");
        }
        Ok(Response::new(ShutdownResponse {}))
    }

    async fn status(&self, r: Request<StatusRequest>) -> Result<Response<StatusResponse>, Status> {
        Self::check_device(&r.get_ref().device_id)?;
        let st = self.aocd.call(json!({"cmd": "status"})).await.map_err(to_status)?;
        Ok(Response::new(StatusResponse { status: Self::statuses(&st) }))
    }

    async fn reset_channel(&self, r: Request<ResetChannelRequest>) -> Result<Response<ResetChannelResponse>, Status> {
        let req = r.get_ref();
        let (fan, n) = self.check_channel(&req.device_id, &req.channel_id).await?;
        let d = self.aocd.desired().await;
        if !d.claimed {
            return Ok(Response::new(ResetChannelResponse {}));
        }
        let others_free = (0..n).filter(|&i| i != fan).all(|i| d.duties.get(i).copied().flatten().is_none());
        if others_free {
            // Nothing left under CoolerControl: give the fans back to AcerOpenControl's mode.
            self.aocd.release().await.map_err(to_status)?;
            log::info!("all fans reset: released to AcerOpenControl");
        } else {
            self.aocd.set_fan(fan, None, n).await.map_err(to_status)?;
        }
        Ok(Response::new(ResetChannelResponse {}))
    }

    async fn enable_manual_fan_control(
        &self,
        r: Request<EnableManualFanControlRequest>,
    ) -> Result<Response<EnableManualFanControlResponse>, Status> {
        let req = r.get_ref();
        self.check_channel(&req.device_id, &req.channel_id).await?;
        if !self.aocd.desired().await.claimed {
            self.aocd.claim().await.map_err(to_status)?;
        }
        Ok(Response::new(EnableManualFanControlResponse {}))
    }

    async fn fixed_duty(&self, r: Request<FixedDutyRequest>) -> Result<Response<FixedDutyResponse>, Status> {
        let req = r.get_ref();
        let (fan, n) = self.check_channel(&req.device_id, &req.channel_id).await?;
        let duty = u8::try_from(req.duty.clamp(0, 100)).unwrap_or(100);
        self.aocd.set_fan(fan, Some(duty), n).await.map_err(to_status)?;
        Ok(Response::new(FixedDutyResponse {}))
    }

    async fn speed_profile(&self, _r: Request<SpeedProfileRequest>) -> Result<Response<SpeedProfileResponse>, Status> {
        Err(Status::unimplemented("no firmware speed profiles; CoolerControl applies profiles itself"))
    }

    async fn lighting(&self, _r: Request<LightingRequest>) -> Result<Response<LightingResponse>, Status> {
        Err(Status::unimplemented("no lighting channels"))
    }

    async fn lcd(&self, _r: Request<LcdRequest>) -> Result<Response<LcdResponse>, Status> {
        Err(Status::unimplemented("no LCD channels"))
    }

    async fn custom_function_one(
        &self,
        _r: Request<CustomFunctionOneRequest>,
    ) -> Result<Response<CustomFunctionOneResponse>, Status> {
        Err(Status::unimplemented("no custom functions"))
    }
}
