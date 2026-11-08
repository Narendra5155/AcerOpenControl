//! Plugin tests against a mock aocd speaking the real JSON-lines protocol.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use tonic::Request;

use crate::aocd::Aocd;
use crate::device_service::v1::device_service_server::DeviceService;
use crate::device_service::v1::*;
use crate::models::v1::{channel_info, status::Metric};
use crate::service::{fan_index, AocService, DEVICE_ID};

static N: AtomicUsize = AtomicUsize::new(0);

struct Mock {
    path: PathBuf,
    log: Arc<Mutex<Vec<Value>>>,
    /// Close the connection after this many requests (simulates an aocd restart).
    drop_after: Arc<Mutex<Option<usize>>>,
}

fn model() -> Value {
    json!({"ok": true, "read_only": false, "version": "0.2.2",
           "model": {"id": "acer-aspire-a715-79g", "name": "Acer Aspire 7 A715-79G"},
           "machine": {"product_name": "Aspire A715-79G"},
           "capabilities": {"fans": 2, "fan_labels": ["CPU", "GPU"]}})
}

fn reply(req: &Value, forbid: bool) -> Value {
    let cmd = req["cmd"].as_str().unwrap_or("");
    if forbid && ["claim_fans", "set_fan_duty", "release_fans"].contains(&cmd) {
        return json!({"ok": false, "code": "forbidden", "error": "permission denied"});
    }
    match cmd {
        "get_model" => model(),
        "status" => json!({"ok": true, "cpu": 71.5, "gpu": null, "read_only": false, "fault": null,
                           "fans": [{"label": "CPU", "duty": 40, "rpm": 3100}, {"label": "GPU", "duty": 35, "rpm": 2900}]}),
        _ => json!({"ok": true}),
    }
}

impl Mock {
    fn start(forbid: bool) -> Mock {
        let path = std::env::temp_dir().join(format!(
            "aoc-cc-mock-{}-{}.sock",
            std::process::id(),
            N.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let log = Arc::new(Mutex::new(Vec::new()));
        let drop_after = Arc::new(Mutex::new(None));
        let (l2, d2) = (log.clone(), drop_after.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let mut out = stream.try_clone().unwrap();
                for (i, line) in BufReader::new(stream).lines().enumerate() {
                    let Ok(line) = line else { break };
                    let req: Value = serde_json::from_str(&line).unwrap();
                    l2.lock().unwrap().push(req.clone());
                    out.write_all(format!("{}\n", reply(&req, forbid)).as_bytes()).unwrap();
                    if d2.lock().unwrap().is_some_and(|n| i + 1 >= n) {
                        *d2.lock().unwrap() = None;
                        break; // drop this connection
                    }
                }
            }
        });
        Mock { path, log, drop_after }
    }

    fn cmds(&self) -> Vec<Value> {
        self.log.lock().unwrap().iter().filter(|r| r["cmd"] != "get_model" && r["cmd"] != "status").cloned().collect()
    }

    fn service(&self) -> AocService {
        AocService::new(Arc::new(Aocd::new(&self.path)), self.path.to_string_lossy())
    }
}

#[test]
fn channel_ids() {
    assert_eq!(fan_index("fan1"), Some(0));
    assert_eq!(fan_index("fan2"), Some(1));
    assert_eq!(fan_index("fan0"), None);
    assert_eq!(fan_index("temp1"), None);
}

#[tokio::test]
async fn lists_one_device_with_fan_and_temp_channels() {
    let mock = Mock::start(false);
    let svc = mock.service();
    let r = svc.list_devices(Request::new(ListDevicesRequest {})).await.unwrap().into_inner();
    assert_eq!(r.devices.len(), 1);
    let d = &r.devices[0];
    assert_eq!(d.id, DEVICE_ID);
    let info = d.info.as_ref().unwrap();
    assert_eq!(info.model.as_deref(), Some("Acer Aspire 7 A715-79G"));
    let fan2 = &info.channels["fan2"];
    assert_eq!(fan2.label.as_deref(), Some("GPU Fan"));
    match &fan2.options {
        Some(channel_info::Options::SpeedOptions(o)) => assert!(o.fixed_enabled && o.max_duty == 100),
        other => panic!("{other:?}"),
    }
    assert_eq!(info.temps.len(), 2);
}

#[tokio::test]
async fn status_maps_fans_and_skips_sleeping_gpu() {
    let mock = Mock::start(false);
    let svc = mock.service();
    let st = svc.status(Request::new(StatusRequest { device_id: DEVICE_ID.into() })).await.unwrap().into_inner();
    let ids: Vec<&str> = st.status.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(ids, ["temp1", "fan1", "fan2"], "gpu temp is null (asleep), so it is left out");
    match &st.status[1].metric {
        Some(Metric::Speed(s)) => assert_eq!((s.duty, s.rpm), (Some(40.0), Some(3100))),
        other => panic!("{other:?}"),
    }
    assert!(svc.status(Request::new(StatusRequest { device_id: "nope".into() })).await.is_err());
}

#[tokio::test]
async fn fixed_duty_claims_then_sets_and_reset_releases() {
    let mock = Mock::start(false);
    let svc = mock.service();
    let set = |ch: &str, duty: i32| FixedDutyRequest { device_id: DEVICE_ID.into(), channel_id: ch.into(), duty };
    svc.fixed_duty(Request::new(set("fan1", 55))).await.unwrap();
    svc.fixed_duty(Request::new(set("fan2", 140))).await.unwrap(); // clamped
    assert_eq!(
        mock.cmds(),
        vec![
            json!({"cmd": "claim_fans", "holder": "CoolerControl"}),
            json!({"cmd": "set_fan_duty", "fan": 0, "duty": 55}),
            json!({"cmd": "set_fan_duty", "fan": 1, "duty": 100}),
        ]
    );
    let reset = |ch: &str| ResetChannelRequest { device_id: DEVICE_ID.into(), channel_id: ch.into() };
    svc.reset_channel(Request::new(reset("fan1"))).await.unwrap();
    assert_eq!(mock.cmds().last().unwrap(), &json!({"cmd": "set_fan_duty", "fan": 0, "duty": null}));
    svc.reset_channel(Request::new(reset("fan2"))).await.unwrap();
    assert_eq!(mock.cmds().last().unwrap(), &json!({"cmd": "release_fans"}), "last fan reset releases the lease");
    assert!(svc.fixed_duty(Request::new(set("fan3", 10))).await.is_err());
}

#[tokio::test]
async fn reconnect_restores_lease_and_duties() {
    let mock = Mock::start(false);
    let svc = mock.service();
    let set = |ch: &str, duty: i32| FixedDutyRequest { device_id: DEVICE_ID.into(), channel_id: ch.into(), duty };
    svc.fixed_duty(Request::new(set("fan1", 60))).await.unwrap();
    *mock.drop_after.lock().unwrap() = Some(1); // the next request is the last on this connection
    svc.status(Request::new(StatusRequest { device_id: DEVICE_ID.into() })).await.unwrap();
    mock.log.lock().unwrap().clear();
    svc.status(Request::new(StatusRequest { device_id: DEVICE_ID.into() })).await.unwrap();
    let after: Vec<Value> = mock.log.lock().unwrap().clone();
    assert_eq!(after[0], json!({"cmd": "claim_fans", "holder": "CoolerControl"}));
    assert_eq!(after[1], json!({"cmd": "set_fan_duty", "fan": 0, "duty": 60}));
    assert_eq!(after[2]["cmd"], "status");
}

#[tokio::test]
async fn forbidden_explains_the_group() {
    let mock = Mock::start(true);
    let svc = mock.service();
    let r = svc
        .enable_manual_fan_control(Request::new(EnableManualFanControlRequest {
            device_id: DEVICE_ID.into(),
            channel_id: "fan1".into(),
        }))
        .await;
    let e = r.unwrap_err();
    assert_eq!(e.code(), tonic::Code::PermissionDenied);
    assert!(e.message().contains("lease_users"));
}

#[tokio::test]
async fn health_reports_unreachable_daemon() {
    let svc = AocService::new(Arc::new(Aocd::new("/nonexistent/aocd.sock")), "/nonexistent/aocd.sock");
    let h = svc.health(Request::new(HealthRequest {})).await.unwrap().into_inner();
    assert_eq!(h.status, health_response::Status::Error as i32);
}
