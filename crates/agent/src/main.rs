//! booty-call agent: runs on each OS partition of a target box.
//!
//! Exposes exactly two authenticated endpoints to the control plane:
//!   GET  /status  -> who am I
//!   POST /reboot  -> set BootNext to the requested OS, reboot
//!
//! No other functionality is exposed; liveness is the control polling /status.

mod boot;

use anyhow::{Context, Result};
use common::{AgentConfig, RebootRequest, Status, whois_node};
use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{IpAddr, ToSocketAddrs, TcpStream};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tiny_http::{Header, Method, Request, Response, Server};

const DEFAULT_CONFIG_PATH: &str =
    if cfg!(target_os = "windows") { r"C:\ProgramData\booty-call\agent.json" } else { "/etc/booty-call/agent.json" };

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

    const SERVICE_NAME: &str = "bootycallagent";

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
                    println!("booty-call-agent: stop requested");
                    std::process::exit(0);
                }
                ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                _ => ServiceControlHandlerResult::NotImplemented,
            }
        };
        let status = match service_control_handler::register(SERVICE_NAME, event_handler) {
            Ok(h) => h,
            Err(e) => {
                eprintln!("booty-call-agent: service register: {e}");
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
            eprintln!("booty-call-agent: {e:#}");
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
        eprintln!("booty-call-agent: {e:#}");
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

    // No tailscale, no service: the peer check needs a tailnet source IP and
    // the agent must never be reachable from the LAN unauthenticated, so we
    // wait for the tailnet interface instead of falling back to 0.0.0.0.
    let server = loop {
        if let Some(ip) = tailscale_ip4() {
            let addr = format!("{}:{}", ip, cfg.port);
            match Server::http(&addr) {
                Ok(s) => break s,
                Err(e) => eprintln!("booty-call-agent: binding to {addr}: {e}; retrying"),
            }
        } else {
            eprintln!("booty-call-agent: no tailscale IPv4 yet, waiting");
        }
        std::thread::sleep(Duration::from_secs(5));
    };
    let status = Arc::new(build_status(&cfg));
    let peers = Arc::new(Mutex::new(HashMap::new()));
    println!(
        "booty-call-agent: box={} os={} listening on {} (peers: {:?})",
        cfg.box_id, cfg.os_id, server.server_addr(), cfg.allowed_peers
    );

    if let Some(control) = cfg.control.clone() {
        let cfg = Arc::clone(&cfg);
        let status = Arc::clone(&status);
        std::thread::spawn(move || register_loop(cfg, control, status));
    }

    for request in server.incoming_requests() {
        let cfg = Arc::clone(&cfg);
        let status = Arc::clone(&status);
        let peers = Arc::clone(&peers);
        std::thread::spawn(move || handle(request, cfg, status, peers));
    }
    Ok(())
}

fn build_status(cfg: &AgentConfig) -> Status {
    let hostname = hostname::get()
        .map(|h| h.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ts_ip = tailscale_ip4().map(|ip| ip.to_string()).unwrap_or_default();
    let ts_hostname = whois_node(&ts_ip).map(|(_, full)| full).unwrap_or_default();
    Status {
        box_id: cfg.box_id.clone(),
        os_id: cfg.os_id.clone(),
        ts_hostname,
        ts_ip,
        hostname,
    }
}

const PEER_TTL: Duration = Duration::from_secs(60);

fn peer_ok(peers: &Arc<Mutex<HashMap<String, (bool, Instant)>>>, allowed: &[String], ip: &str) -> bool {
    if let Some((ok, at)) = peers.lock().unwrap().get(ip) {
        if at.elapsed() < PEER_TTL {
            return *ok;
        }
    }
    let ok = common::peer_allowed(allowed, ip);
    peers.lock().unwrap().insert(ip.to_string(), (ok, Instant::now()));
    ok
}

fn handle(
    mut request: Request,
    cfg: Arc<AgentConfig>,
    status: Arc<Status>,
    peers: Arc<Mutex<HashMap<String, (bool, Instant)>>>,
) {
    let route = request.url().split('?').next().unwrap_or("/").to_string();
    let src = request
        .remote_addr()
        .map(|a| a.ip().to_string())
        .unwrap_or_default(); // unknown source: whois will fail, request 403s

    let (code, body) = match (request.method(), route.as_str()) {
        (Method::Get, "/status") if peer_ok(&peers, &cfg.allowed_peers, &src) => (
            200,
            serde_json::to_string(&*status).unwrap_or_default(),
        ),
        (Method::Post, "/reboot") if peer_ok(&peers, &cfg.allowed_peers, &src) => {
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
        _ => (403, r#"{"error":"peer not allowed"}"#.into()),
    };

    let header = Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).unwrap();
    let _ = request.respond(Response::from_string(body).with_status_code(code).with_header(header));
}

/// Registers this agent with the control and keeps retrying until it
/// succeeds: the control may not be reachable yet at install/boot time, and
/// re-registration also heals a node entry that was deleted from the control.
fn register_loop(cfg: Arc<AgentConfig>, control: String, status: Arc<Status>) {
    const RETRY: Duration = Duration::from_secs(60);
    loop {
        match post_register(&control, &cfg, &status) {
            Ok(()) => {
                println!("booty-call-agent: registered with control at {control}");
                return;
            }
            Err(e) => eprintln!("booty-call-agent: registering with {control}: {e:#}; retrying"),
        }
        std::thread::sleep(RETRY);
    }
}

fn post_register(control: &str, cfg: &AgentConfig, status: &Status) -> Result<()> {
    if status.ts_ip.is_empty() {
        anyhow::bail!("no tailscale IPv4 yet");
    }
    let (host, port) = match control.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().context("invalid control port")?),
        None => (control.to_string(), 8765),
    };
    let mut addrs = (host.as_str(), port)
        .to_socket_addrs()
        .with_context(|| format!("resolving control host {host}"))?;
    let addr = addrs.next().context("no address for control host")?;
    let body = serde_json::json!({ "os_id": cfg.os_id, "ts_ip": status.ts_ip, "port": cfg.port }).to_string();
    let path = format!("/api/boxes/{}/nodes", cfg.box_id);
    let mut sock =
        TcpStream::connect_timeout(&addr, Duration::from_secs(5)).with_context(|| format!("connecting to {addr}"))?;
    sock.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let req = format!(
        "POST {path} HTTP/1.1\r\nHost: {host}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    sock.write_all(req.as_bytes())?;
    let mut resp = String::new();
    sock.read_to_string(&mut resp)?;
    let first = resp.lines().next().unwrap_or_default();
    if first.contains(" 200") {
        Ok(())
    } else {
        anyhow::bail!("control responded: {first} {}", resp.lines().nth(1).unwrap_or_default())
    }
}

/// Sets BootNext for `os` and reboots. Does not return on success.
fn reboot_to(cfg: &AgentConfig, os: &str) -> Result<()> {
    let entry = cfg
        .boot_entry(os)
        .ok_or_else(|| anyhow::anyhow!("no boot entry configured for os {os:?}"))?;
    boot::set_bootnext(entry)?;
    println!("booty-call-agent: BootNext={entry}, rebooting into {os:?}");
    // Give the HTTP response a moment to flush before the machine goes away.
    std::thread::sleep(Duration::from_millis(500));
    boot::reboot();
}

fn tailscale_ip4() -> Option<IpAddr> {
    let out = Command::new("tailscale")
        .args(["ip", "-4"])
        .output()
        .ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    s.parse().ok()
}


