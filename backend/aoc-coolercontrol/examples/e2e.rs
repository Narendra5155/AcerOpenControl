//! End-to-end check playing CoolerControl's part over real gRPC.
//!   cargo run -p aoc-coolercontrol --example e2e -- /tmp/aoc-coolercontrol.sock
//! Drives the same RPC sequence coolercontrold uses: Health, ListDevices,
//! Status, EnableManualFanControl, FixedDuty, ResetChannel, Shutdown.

use hyper_util::rt::TokioIo;
use tokio::net::UnixStream;
use tonic::transport::{Endpoint, Uri};
use tower::service_fn;

pub mod models {
    pub mod v1 {
        tonic::include_proto!("coolercontrol.models.v1");
    }
}
pub mod device_service {
    pub mod v1 {
        tonic::include_proto!("coolercontrol.device_service.v1");
    }
}
use device_service::v1::device_service_client::DeviceServiceClient;
use device_service::v1::*;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).unwrap_or_else(|| "/tmp/aoc-coolercontrol.sock".into());
    let channel = Endpoint::try_from("http://[::]:50051")?
        .connect_with_connector(service_fn(move |_: Uri| {
            let p = path.clone();
            async move { Ok::<_, std::io::Error>(TokioIo::new(UnixStream::connect(p).await?)) }
        }))
        .await?;
    let mut c = DeviceServiceClient::new(channel);
    let dev = "aoc".to_string();

    let h = c.health(HealthRequest {}).await?.into_inner();
    println!("Health: {} {} status={}", h.name, h.version, h.status);
    let l = c.list_devices(ListDevicesRequest {}).await?.into_inner();
    for d in &l.devices {
        let info = d.info.as_ref().unwrap();
        let mut ch: Vec<_> =
            info.channels.iter().map(|(k, v)| format!("{k}={}", v.label.clone().unwrap_or_default())).collect();
        ch.sort();
        println!("Device: id={} model={:?} channels={ch:?} temps={}", d.id, info.model, info.temps.len());
    }
    let show =
        |s: StatusResponse| s.status.iter().map(|x| format!("{}:{:?}", x.id, x.metric)).collect::<Vec<_>>().join("  ");
    println!("Status: {}", show(c.status(StatusRequest { device_id: dev.clone() }).await?.into_inner()));
    c.enable_manual_fan_control(EnableManualFanControlRequest { device_id: dev.clone(), channel_id: "fan1".into() })
        .await?;
    c.fixed_duty(FixedDutyRequest { device_id: dev.clone(), channel_id: "fan1".into(), duty: 80 }).await?;
    c.fixed_duty(FixedDutyRequest { device_id: dev.clone(), channel_id: "fan2".into(), duty: 35 }).await?;
    println!("FixedDuty fan1=80 fan2=35 sent");
    tokio::time::sleep(std::time::Duration::from_millis(1800)).await; // one aocd tick
    println!("Status: {}", show(c.status(StatusRequest { device_id: dev.clone() }).await?.into_inner()));
    c.reset_channel(ResetChannelRequest { device_id: dev.clone(), channel_id: "fan2".into() }).await?;
    println!("ResetChannel fan2 sent");
    if let Err(e) = c.fixed_duty(FixedDutyRequest { device_id: dev.clone(), channel_id: "fan9".into(), duty: 1 }).await
    {
        println!("FixedDuty fan9 rejected as expected: {:?}", e.code());
    }
    c.shutdown(ShutdownRequest {}).await?;
    println!("Shutdown sent");
    Ok(())
}
