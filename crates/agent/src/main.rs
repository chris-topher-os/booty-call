//! oswitch agent: runs on each OS partition of a target box.
//!
//! Exposes exactly two authenticated endpoints to the control plane:
//!   GET  /status  -> who am I
//!   POST /reboot  -> set BootNext to the requested OS, reboot
//!
//! No other functionality is exposed; liveness is the control polling /status.

mod boot;

use anyhow::Result;
use common::{AgentConfig, RebootRequest, Status};
use std::net::{IpAddr, Ipv4Addr};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;
use tiny_http::{Header, Method, Request, Response, Server};

const DEFAULT_CONFIG_PATH: &str =
    if cfg!(target_os = "windows") { r"C:\ProgramData\oswitch\agent.json" } else { "/etc/oswitch/agent.json" };

#[cfg(target_os = "windows")]
mod win {
    use std::ffi::OsString;
    use std::time::Duration;
    use windows_service::define_windows_service;
    use windows_service::service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    };
    use windows_service::service_control_handler::{self, ServiceControlHandlerResult};
    use windows_service::service_dispatcher;

    const SERVICE_NAME: &str = "oswitchagent";

    define_windows_service!(ffi_service_main, service_main);

    /// Returns true when launched by the SCM (main thread then blocks until
    /// the service stops). False = console run.
    pub fn run_as_service() -> bool {
        match service_dispatcher::start(SERVICE_NAME, ffi_service_main) {
            Ok(()) => true,
            Err(_) => false,
        }
    }

    fn service_main(_args: Vec<OsString>) {
        let event_handler = |control_event| -> ServiceControlHandlerResult {
            match control_event {
                ServiceControl::Stop => {
                    println!("oswitch-agent: stop requested");
                    std::process::exit(0);
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };
        let status = match service_control_handler::register(SERVICE_NAME, event_handler) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("oswitch-agent: service register: {e}");
                std::process::exit(1);
            }
        };
        let _ = status.set_service_status(ServiceStatus {
            service_type: ServiceType::OWN_PROCESS,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::empty(),
            exit_code: ServiceExitCode::NO_ERROR,
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        });
        if let Err(e) = super::run_agent() {
            eprintln!("oswitch-agent: {e:#}");
            std::process::exit(1);
        }
    }
}

fn main() {
    // When launched via `sc`/services.msc, the SCM runs us as a service.
    // `service_dispatcher::start` fails when run from a console, in which
    // case we just run in the foreground.
    #[cfg(target_os = "windows")]
    if win::run_as_service() {
        return;
    }
    if let Err(e) = run_agent() {
        eprintln!("oswitch-agent: {e:#}");
        std::process::exit(1);
    }
}

fn config_path() -> String {
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        if a == "--config" {
            if let Some(p) = args.next() {
                return p;
            }
        }
        if let Some(p) = a.strip_prefix("--config=") {
            return p.to_string();
        }
    }
    DEFAULT_CONFIG_PATH.to_string()
}

fn run_agent() -> Result<()> {
    let path = config_path();
    let cfg = Arc::new(AgentConfig::load(&path)?);
    let status = Arc::new(build_status(&cfg));

    let bind_ip = match tailscale_ip4() {
        Some(ip) => ip,
        None => {
            eprintln!("oswitch-agent: warning: no tailscale IPv4 found; binding to 0.0.0.0 (token still required)");
            IpAddr::V4(Ipv4Addr::UNSPECIFIED)
        }
    };
    let addr = format!("{}:{}", bind_ip, cfg.port);
    let server = Server::http(&addr).map_err(|e| anyhow::anyhow!("binding to {addr}: {e}"))?;
    println!(
        "oswitch-agent: box={} os={} listening on {}",
        cfg.box_id, cfg.os_id, addr
    );

    for request in server.incoming_requests() {
        let cfg = Arc::clone(&cfg);
        let status = Arc::clone(&status);
        std::thread::spawn(move || handle(request, cfg, status));
    }
    Ok(())
}

fn build_status(cfg: &AgentConfig) -> Status {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default();
    Status {
        box_id: cfg.box_id.clone(),
        os_id: cfg.os_id.clone(),
        ts_hostname: tailscale_node_name(),
        ts_ip: tailscale_ip4().map(|ip| ip.to_string()).unwrap_or_default(),
        hostname,
    }
}

fn handle(mut request: Request, cfg: Arc<AgentConfig>, status: Arc<Status>) {
    let route = request.url().split('?').next().unwrap_or("/").to_string();
    let auth = request
        .headers()
        .iter()
        .find(|h| h.field.as_str() == "Authorization")
        .map(|h| h.value.as_str().to_string());

    let (code, body) = match (request.method(), route.as_str()) {
        (Method::Get, "/status") if bearer_ok(&auth, &cfg.token) => (
            200,
            serde_json::to_string(&*status).unwrap_or_default(),
        ),
        (Method::Post, "/reboot") if bearer_ok(&auth, &cfg.token) => {
            let mut body = String::new();
            if request.as_reader().read_to_string(&mut body).is_err() {
                (400, r#"{"error":"unreadable body"}"#.into())
            } else {
                match serde_json::from_str::<RebootRequest>(&body) {
                    Ok(req) if cfg.boot_entry(&req.os).is_none() => (
                        400,
                        format!(r#"{{"error":"no boot entry for os {:?}"}}"#, req.os),
                    ),
                    Ok(req) => match reboot_to(&cfg, &req.os) {
                        Ok(()) => (202, r#"{"rebooting":true}"#.into()),
                        Err(e) => (500, format!(r#"{{"error":"{}"}}"#, e.to_string().replace('"', "'"))),
                    },
                    Err(_) => (400, r#"{"error":"expected {\"os\":\"<os_id>\"}"}"#.into()),
                }
            }
        }
        _ => (401, r#"{"error":"unauthorized"}"#.into()),
    };

    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let _ = request.respond(Response::from_string(body).with_status_code(code).with_header(header));
}

/// Sets BootNext for `os` and reboots. Does not return on success.
fn reboot_to(cfg: &AgentConfig, os: &str) -> Result<()> {
    let entry = cfg
        .boot_entry(os)
        .ok_or_else(|| anyhow::anyhow!("no boot entry configured for os {os:?}"))?;
    boot::set_bootnext(entry)?;
    println!("oswitch-agent: BootNext={entry}, rebooting into {os:?}");
    // Give the HTTP response a moment to flush before the machine goes away.
    std::thread::sleep(Duration::from_millis(500));
    boot::reboot();
}

fn bearer_ok(auth: &Option<String>, token: &str) -> bool {
    auth.as_deref()
        .and_then(|a| a.strip_prefix("Bearer "))
        .is_some_and(|t| t == token)
}

fn tailscale_ip4() -> Option<IpAddr> {
    let out = Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    s.parse().ok()
}

fn tailscale_node_name() -> String {
    let out = match Command::new("tailscale")
        .args(["status", "--json"])
        .output()
        .ok()
    {
        Some(o) => o,
        None => return String::new(),
    };
    let v: serde_json::Value = match serde_json::from_slice(&out.stdout).ok() {
        Some(v) => v,
        None => return String::new(),
    };
    v.get("Self")
        .and_then(|s| s.get("Name"))
        .and_then(|n| n.as_str())
        .unwrap_or_default()
        .to_string()
}
