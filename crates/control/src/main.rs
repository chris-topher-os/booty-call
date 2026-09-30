//! booty-call control plane: node registry, liveness polling, switch state
//! machine, and the PWA. Runs on the control machine as a daemon.

mod wol;

use anyhow::{Context, Result};
use axum::extract::{ConnectInfo, Path, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{delete, get, post};
use axum::Json;
use common::{BoxPhase, BoxView, NodeView, RebootRequest, StateResponse, Status, SwitchStage};
use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;
use tower_http::services::ServeDir;
use tracing::{info, warn};

const DEFAULT_CONFIG_PATH: &str = "~/.config/booty-call/control.json";
const CONTROL_PORT: u16 = 8765;
const NODE_POLL_SECS: f64 = 3.0;
const NODE_REQ_TIMEOUT: Duration = Duration::from_secs(2);
const WAKE_WAIT: Duration = Duration::from_secs(120);
const REBOOT_WAIT: Duration = Duration::from_secs(240);
const STATE_TICK: Duration = Duration::from_secs(5);

// ---------------------------------------------------------------- config

#[derive(Deserialize, Clone)]
struct Config {
    /// e.g. "100.64.0.2:8765". Empty = tailscale IP :8765, fallback 127.0.0.1.
    #[serde(default)]
    listen: String,
    #[serde(default = "default_state_file")]
    state_file: String,
    #[serde(default = "default_poll")]
    poll_interval_secs: f64,
    boxes: Vec<BoxCfg>,
}

fn default_state_file() -> String {
    "~/.local/share/booty-call/boxes.json".into()
}
fn default_poll() -> f64 {
    NODE_POLL_SECS
}

#[derive(Deserialize, Clone)]
struct BoxCfg {
    id: String,
    name: String,
    /// OS the firmware boots when the box cold-boots (drives the WoL flow).
    default_os: String,
    #[serde(default)]
    wol: Option<wol::WolTarget>,
}

impl Config {
    fn load(path: &str) -> Result<Self> {
        let raw = std::fs::read_to_string(path).with_context(|| format!("reading config {path}"))?;
        let cfg: Config =
            serde_json::from_str(&raw).with_context(|| format!("parsing config {path}"))?;
        Ok(cfg)
    }
}

// ----------------------------------------------------------------- store

/// A registered switch node: one per OS partition.
#[derive(Default)]
struct Store {
    /// key: "{box_id}/{os_id}"
    nodes: BTreeMap<String, StoredNode>,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq)]
struct StoredNode {
    ts_ip: String,
    #[serde(default = "default_node_port")]
    port: u16,
}

fn default_node_port() -> u16 {
    8766
}

impl Store {
    fn load(path: &str) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(raw) => Ok(Store {
                nodes: serde_json::from_str(&raw).context("parsing state file")?,
            }),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Store {
                nodes: BTreeMap::new(),
            }),
            Err(e) => Err(e.into()),
        }
    }

    fn save(&self, path: &str) -> Result<()> {
        if let Some(parent) = std::path::Path::new(path).parent() {
            std::fs::create_dir_all(parent)?;
        }
        let raw = serde_json::to_string_pretty(&self.nodes)?;
        std::fs::write(path, raw)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }
}

// --------------------------------------------------------------- runtime

struct Runtime {
    store: Store,
    /// key: "{box_id}/{os_id}"
    live: HashMap<String, bool>,
    last_seen: HashMap<String, Option<i64>>,
    /// per box id
    phase: HashMap<String, BoxPhase>,
}

struct AppState {
    cfg: Arc<Config>,
    rt: Arc<RwLock<Runtime>>,
    client: reqwest::Client,
}

impl AppState {
    fn node_key(box_id: &str, os: &str) -> String {
        format!("{box_id}/{os}")
    }

    fn find_box(&self, box_id: &str) -> Option<&BoxCfg> {
        self.cfg.boxes.iter().find(|b| b.id == box_id)
    }

}

// ----------------------------------------------------------------- http

type ApiError = (StatusCode, Json<serde_json::Value>);

fn err(status: StatusCode, msg: impl ToString) -> ApiError {
    (status, Json(serde_json::json!({ "error": msg.to_string() })))
}

async fn get_state(State(st): State<Arc<AppState>>) -> Result<impl IntoResponse, ApiError> {
    let rt = st.rt.read().await;
    let boxes = st
        .cfg
        .boxes
        .iter()
        .map(|b| {
            let nodes: Vec<NodeView> = rt
                .store
                .nodes
                .iter()
                .filter(|(k, _)| k.starts_with(&format!("{}/", b.id)))
                .map(|(k, _)| {
                    let os = k.split('/').nth(1).unwrap_or_default().to_string();
                    NodeView {
                        os_id: os.clone(),
                        online: rt.live.get(&AppState::node_key(&b.id, &os)).copied().unwrap_or(false),
                        last_seen: rt.last_seen.get(&AppState::node_key(&b.id, &os)).copied().flatten(),
                    }
                })
                .collect();
            let phase = rt.phase.get(&b.id).cloned().unwrap_or(BoxPhase::Idle);
            BoxView {
                id: b.id.clone(),
                name: b.name.clone(),
                default_os: b.default_os.clone(),
                has_wol: b.wol.is_some(),
                phase,
                nodes,
            }
        })
        .collect();
    Ok(Json(StateResponse { boxes }))
}

#[derive(Deserialize)]
struct RegisterReq {
    os_id: String,
    ts_ip: String,
    #[serde(default = "default_node_port")]
    port: u16,
}

/// Idempotent upsert. Nodes self-register at startup by claiming their own
/// tailnet source IP as `ts_ip`; the control trusts the source address and
/// ignores the body value for.
async fn register_node(
    State(st): State<Arc<AppState>>,
    ci: ConnectInfo<SocketAddr>,
    Path(box_id): Path<String>,
    Json(req): Json<RegisterReq>,
) -> Result<impl IntoResponse, ApiError> {
    let ip = ci.0.ip().to_string();
    if req.ts_ip != ip {
        return Err(err(
            StatusCode::FORBIDDEN,
            "ts_ip must match the caller's tailnet address",
        ));
    }
    if st.find_box(&box_id).is_none() {
        return Err(err(StatusCode::NOT_FOUND, format!("unknown box {box_id:?}")));
    }
    if req.os_id.is_empty() {
        return Err(err(StatusCode::BAD_REQUEST, "os_id is required"));
    }
    let mut rt = st.rt.write().await;
    let key = AppState::node_key(&box_id, &req.os_id);
    let node = StoredNode { ts_ip: ip, port: req.port };
    if rt.store.nodes.get(&key) != Some(&node) {
        rt.store.nodes.insert(key.clone(), node);
        rt.store.save(&st.cfg.state_file)
            .map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("saving state: {e}")))?;
        info!(%key, "registered node");
    }
    Ok(Json(serde_json::json!({ "os_id": req.os_id })))
}

async fn unregister_node(
    State(st): State<Arc<AppState>>,
    Path((box_id, os_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ApiError> {
    let mut rt = st.rt.write().await;
    let key = AppState::node_key(&box_id, &os_id);
    if rt.store.nodes.remove(&key).is_none() {
        return Err(err(StatusCode::NOT_FOUND, "node not registered"));
    }
    rt.live.remove(&key);
    rt.last_seen.remove(&key);
    rt.store.save(&st.cfg.state_file).map_err(|e| err(StatusCode::INTERNAL_SERVER_ERROR, format!("saving state: {e}")))?;
    info!(%key, "unregistered node");
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct SwitchReq {
    os: String,
}

async fn do_switch_api(
    State(st): State<Arc<AppState>>,
    Path(box_id): Path<String>,
    Json(req): Json<SwitchReq>,
) -> Result<impl IntoResponse, ApiError> {
    if st.find_box(&box_id).is_none() {
        return Err(err(StatusCode::NOT_FOUND, format!("unknown box {box_id:?}")));
    }
    {
        let rt = st.rt.read().await;
        if !rt.store.nodes.contains_key(&AppState::node_key(&box_id, &req.os)) {
            let os = req.os;
            return Err(err(StatusCode::NOT_FOUND, format!("unregistered os {os:?}")));
        }
        match rt.phase.get(&box_id) {
            None | Some(BoxPhase::Idle) => {}
            Some(BoxPhase::Switching { target, .. }) => {
                return Err(err(StatusCode::CONFLICT, format!("switch in progress to {target}")))
            }
            Some(BoxPhase::Stuck { target, error }) => {
                return Err(err(StatusCode::CONFLICT, format!("box stuck: {error} (retry to clear, target was {target})")))
            }
        }
    }
    let st = Arc::clone(&st);
    tokio::spawn(switch_task(st, box_id, req.os));
    Ok(StatusCode::ACCEPTED)
}

// ------------------------------------------------------------- poller

async fn poller(st: Arc<AppState>) {
    let interval = Duration::from_secs_f64(st.cfg.poll_interval_secs);
    loop {
        tokio::time::sleep(interval).await;
        let mut rt = st.rt.write().await;
        let mut dirty = false;
        // Clone entries: polling may update ts_ip in the map mid-iteration.
        let entries: Vec<(String, StoredNode)> = rt
            .store
            .nodes
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();
        for (key, node) in entries {
            let url = format!("http://{}:{}/status", node.ts_ip, node.port);
            let resp = st
                .client
                .get(&url)
                .send()
                .await
                .ok()
                .filter(|r| r.status().is_success());
            let status = match resp {
                Some(r) => r.json::<Status>().await.ok(),
                None => None,
            };
            let ok = match status {
                Some(s) => {
                    // Tailnet IPs can change; trust the agent's own report.
                    if !s.ts_ip.is_empty() && s.ts_ip != node.ts_ip {
                        warn!(%key, old = %node.ts_ip, new = %s.ts_ip, "tailnet IP changed, updating registry");
                        let mut updated = node.clone();
                        updated.ts_ip = s.ts_ip;
                        rt.store.nodes.insert(key.clone(), updated);
                        dirty = true;
                    }
                    true
                }
                None => false,
            };
            rt.live.insert(key.clone(), ok);
            if ok {
                let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() as i64;
                rt.last_seen.insert(key.clone(), Some(now));
            }
        }
        // A stuck box recovers as soon as any of its agents answers again.
        let recovered: Vec<String> = rt
            .phase
            .iter()
            .filter(|(_, p)| matches!(p, BoxPhase::Stuck { .. }))
            .filter_map(|(box_id, _)| {
                let any_live = rt.store.nodes.keys().any(|k| {
                    k.starts_with(&format!("{box_id}/")) && rt.live.get(k) == Some(&true)
                });
                any_live.then(|| box_id.clone())
            })
            .collect();
        for box_id in recovered {
            info!(%box_id, "box recovered, clearing stuck");
            rt.phase.insert(box_id, BoxPhase::Idle);
        }
        if dirty {
            if let Err(e) = rt.store.save(&st.cfg.state_file) {
                warn!(error = %e, "saving state file");
            }
        }
        drop(rt);
    }
}

// ------------------------------------------------------- switch machine

async fn set_phase(st: &AppState, box_id: &str, phase: BoxPhase) {
    let mut rt = st.rt.write().await;
    rt.phase.insert(box_id.to_string(), phase);
}

async fn live_oses(st: &AppState, box_id: &str) -> Vec<String> {
    let rt = st.rt.read().await;
    rt.store
        .nodes
        .keys()
        .filter(|k| k.starts_with(&format!("{box_id}/")))
        .filter_map(|k| {
            rt.live.get(k).copied().unwrap_or(false).then(|| {
                k.split_once('/')
                    .map(|(_, os)| os.to_string())
                    .unwrap_or_default()
            })
        })
        .collect()
}

async fn request_reboot(st: &AppState, box_id: &str, from: &str, target: &str) -> bool {
    let Some(node) = st.rt.read().await.store.nodes.get(&AppState::node_key(box_id, from)).cloned() else {
        return false;
    };
    let url = format!("http://{}:{}/reboot", node.ts_ip, node.port);
    st.client
        .post(&url)
        .json(&RebootRequest { os: target.to_string() })
        .send()
        .await
        .ok()
        .map(|r| r.status().is_success())
        .unwrap_or(false)
}

/// Wait until `target` comes online. If some *other* OS comes up instead,
/// issue one corrective reboot (covers BootNext being ignored or the target
/// entry failing and firmware falling through to the default).
async fn wait_for_target(st: &AppState, box_id: &str, target: &str, deadline: Duration) {
    let mut retried = false;
    let until = Instant::now() + deadline;
    loop {
        let live = live_oses(st, box_id).await;
        if live.iter().any(|o| o == target) {
            info!(%box_id, %target, "switch complete");
            set_phase(st, box_id, BoxPhase::Idle).await;
            return;
        }
        if let Some(other) = live.first() {
            if !retried {
                retried = true;
                info!(%box_id, %other, %target, "unexpected OS online, corrective reboot");
                if request_reboot(st, box_id, other, target).await {
                    continue;
                }
            }
            set_phase(
                st,
                box_id,
                BoxPhase::Stuck {
                    target: target.to_string(),
                    error: format!("box came up in {other:?} but {target:?} never came online"),
                },
            )
            .await;
            return;
        }
        if Instant::now() > until {
            set_phase(
                st,
                box_id,
                BoxPhase::Stuck {
                    target: target.to_string(),
                    error: format!("timed out waiting for {target:?} to come online"),
                },
            )
            .await;
            return;
        }
        tokio::time::sleep(STATE_TICK).await;
    }
}

async fn switch_task(st: Arc<AppState>, box_id: String, target: String) {
    let Some(box_cfg) = st.find_box(&box_id).map(|b| b.clone()) else {
        return;
    };

    // Case 1: some OS is up.
    if let Some(from) = live_oses(&st, &box_id).await.into_iter().next() {
        if from == target {
            set_phase(&st, &box_id, BoxPhase::Idle).await;
            return;
        }
        set_phase(
            &st,
            &box_id,
            BoxPhase::Switching { target: target.clone(), stage: SwitchStage::AwaitingReboot },
        )
        .await;
        if !request_reboot(&st, &box_id, &from, &target).await {
            set_phase(
                &st,
                &box_id,
                BoxPhase::Stuck {
                    target: target.clone(),
                    error: format!("reboot request to {from:?} failed"),
                },
            )
            .await;
            return;
        }
        wait_for_target(&st, &box_id, &target, REBOOT_WAIT).await;
        return;
    }

    // Case 2: box is offline; wake it (it boots its default OS).
    let Some(wol) = &box_cfg.wol else {
        set_phase(
            &st,
            &box_id,
            BoxPhase::Stuck {
                target,
                error: "box is offline and no wake-on-LAN is configured".into(),
            },
        )
        .await;
        return;
    };
    set_phase(
        &st,
        &box_id,
        BoxPhase::Switching { target: target.clone(), stage: SwitchStage::Waking },
    )
    .await;
    if let Err(e) = wol::send_wol(wol) {
        set_phase(&st, &box_id, BoxPhase::Stuck { target, error: format!("wake failed: {e:#}") }).await;
        return;
    }
    // Wait for any OS to appear (expected: the default).
    let mut woke = None;
    let until = Instant::now() + WAKE_WAIT;
    while Instant::now() < until {
        if let Some(os) = live_oses(&st, &box_id).await.into_iter().next() {
            woke = Some(os);
            break;
        }
        tokio::time::sleep(STATE_TICK).await;
    }
    let Some(os) = woke else {
        set_phase(
            &st,
            &box_id,
            BoxPhase::Stuck { target, error: "no response to wake-on-LAN".into() },
        )
        .await;
        return;
    };
    if os == target {
        info!(%box_id, "woke directly into target");
        set_phase(&st, &box_id, BoxPhase::Idle).await;
        return;
    }
    set_phase(
        &st,
        &box_id,
        BoxPhase::Switching { target: target.clone(), stage: SwitchStage::AwaitingReboot },
    )
    .await;
    if !request_reboot(&st, &box_id, &os, &target).await {
        set_phase(
            &st,
            &box_id,
            BoxPhase::Stuck {
                target,
                error: format!("reboot request to {os:?} failed"),
            },
        )
        .await;
        return;
    }
    wait_for_target(&st, &box_id, &target, REBOOT_WAIT).await;
}

// ------------------------------------------------------------------ main

fn expand_home(p: &str) -> String {
    if let Some(rest) = p.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return format!("{}/{}", home.to_string_lossy(), rest);
        }
    }
    p.to_string()
}

fn config_path() -> String {
    let args: Vec<String> = std::env::args().skip(1).collect();
    for (i, a) in args.iter().enumerate() {
        if let Some(p) = a.strip_prefix("--config=") {
            return expand_home(p);
        }
        if a == "--config" && i + 1 < args.len() {
            return expand_home(&args[i + 1]);
        }
    }
    expand_home(DEFAULT_CONFIG_PATH)
}

fn tailscale_ip4() -> Option<IpAddr> {
    let out = Command::new("tailscale").args(["ip", "-4"]).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    s.parse().ok()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "booty_call_control=info".into()),
        )
        .init();

    let cfg_path = config_path();
    let cfg = Arc::new(Config::load(&cfg_path)?);
    info!(path = %cfg_path, boxes = cfg.boxes.len(), "loaded config");

    let store = Store::load(&expand_home(&cfg.state_file))
        .with_context(|| format!("loading state file {}", cfg.state_file))?;
    let rt = Arc::new(RwLock::new(Runtime {
        store,
        live: HashMap::new(),
        last_seen: HashMap::new(),
        phase: HashMap::new(),
    }));
    let client = reqwest::Client::builder()
        .timeout(NODE_REQ_TIMEOUT)
        .build()
        .context("building http client")?;
    let st = Arc::new(AppState { cfg: Arc::clone(&cfg), rt, client });

    tokio::spawn(poller(Arc::clone(&st)));

    let app = axum::Router::new()
        .route("/api/state", get(get_state))
        .route("/api/boxes/:box_id/nodes", post(register_node))
        .route("/api/boxes/:box_id/nodes/:os_id", delete(unregister_node))
        .route("/api/boxes/:box_id/switch", post(do_switch_api))
        .fallback_service(ServeDir::new("static"))
        .with_state(st);

    let addr = if cfg.listen.is_empty() {
        match tailscale_ip4() {
            Some(ip) => format!("{ip}:{CONTROL_PORT}"),
            None => {
                warn!("no tailscale IPv4 found; listening on 127.0.0.1:{CONTROL_PORT} (set listen in config to expose on the tailnet)");
                format!("127.0.0.1:{CONTROL_PORT}")
            }
        }
    } else {
        cfg.listen.clone()
    };
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| format!("binding to {addr}"))?;
    info!(%addr, "control plane listening");
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}
