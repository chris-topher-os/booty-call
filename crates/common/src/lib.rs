//! Shared wire types for the booty-call agent and control plane.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;

/// `GET /status` response: agent -> control.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Status {
    pub box_id: String,
    pub os_id: String,
    pub hostname: String,
    /// Tailscale node name (e.g. `cogito.tail1234.ts.net`), if resolvable.
    pub ts_hostname: String,
    /// Tailscale IPv4 of this agent, if resolvable.
    pub ts_ip: String,
}

/// `POST /reboot` body: control -> agent.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RebootRequest {
    /// os_id of the partition to boot next.
    pub os: String,
}

/// Liveness of one OS partition as seen by the control.
#[derive(Serialize, Clone, Debug)]
pub struct NodeView {
    pub os_id: String,
    pub online: bool,
    /// Unix seconds of last successful poll, if ever seen.
    pub last_seen: Option<i64>,
}

#[derive(Serialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SwitchStage {
    /// Wake-on-LAN sent; waiting for the box to boot (expected: default OS).
    Waking,
    /// Reboot requested on a live OS; waiting for the target OS agent.
    AwaitingReboot,
}

#[derive(Serialize, Clone, Debug)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum BoxPhase {
    Idle,
    Switching { target: String, stage: SwitchStage },
    Stuck { target: String, error: String },
}

#[derive(Serialize, Clone, Debug)]
pub struct BoxView {
    pub id: String,
    pub name: String,
    pub default_os: String,
    pub has_wol: bool,
    pub phase: BoxPhase,
    pub nodes: Vec<NodeView>,
}

/// `GET /api/state` response: what the PWA renders.
#[derive(Serialize, Clone, Debug)]
pub struct StateResponse {
    pub boxes: Vec<BoxView>,
}

/// Agent configuration. The agent is deliberately dumb: it only serves
/// /status and /reboot; all intelligence lives in the control.
#[derive(Deserialize, Clone, Debug)]
pub struct AgentConfig {
    pub box_id: String,
    pub os_id: String,
    /// Tailscale node names (short name or FQDN) allowed to talk to this
    /// agent. Identity is resolved from the source IP via `tailscale whois`.
    pub allowed_peers: Vec<String>,
    /// os_id -> UEFI boot entry number (the BootNext value).
    pub boot_entries: BTreeMap<String, u16>,
    #[serde(default = "default_port")]
    pub port: u16,
    /// Control plane address (`host:port`, or `host` for the default port
    /// 8765). Host may be a tailnet FQDN or IP. When set, the agent
    /// registers itself with the control at startup; when absent, the node
    /// must be registered externally.
    #[serde(default)]
    pub control: Option<String>,
}

/// Identify a tailnet IP: `(short_name, fqdn)`, e.g.
/// `("fabrico", "fabrico.tailnet.ts.net")`. None for non-tailnet addresses
/// or when tailscale itself can't answer.
pub fn whois_node(ip: &str) -> Option<(String, String)> {
    let out = std::process::Command::new("tailscale")
        .args(["whois", "--json", ip])
        .output()
        .ok()?;
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
    let name = v.get("Node")?.get("Name")?.as_str()?;
    let full = name.trim_end_matches('.').to_string();
    let short = full.split('.').next()?.to_string();
    Some((short, full))
}

/// True if `ip` resolves (via `tailscale whois`) to a node whose short or
/// full name is in `allowed`. Non-tailnet sources never match.
pub fn peer_allowed(allowed: &[String], ip: &str) -> bool {
    whois_node(ip)
        .is_some_and(|(short, full)| allowed.iter().any(|a| a == &short || a == &full))
}

fn default_port() -> u16 {
    8766
}

impl AgentConfig {
    pub fn load(path: &str) -> Result<Self> {
        let raw = fs::read_to_string(path)
            .with_context(|| format!("reading agent config {}", path))?;
        let cfg: AgentConfig =
            serde_json::from_str(&raw).with_context(|| format!("parsing agent config {}", path))?;
        Ok(cfg)
    }

    pub fn boot_entry(&self, os: &str) -> Option<u16> {
        self.boot_entries.get(os).copied()
    }
}
