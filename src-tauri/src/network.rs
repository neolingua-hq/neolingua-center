use if_addrs::{get_if_addrs, IfAddr};
use serde::Serialize;
use std::net::Ipv4Addr;
use std::process::Command;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkInfo {
    pub addresses: Vec<String>,
    /// e.g. MacBook.local (macOS Bonjour)
    pub host_name: Option<String>,
    pub localhost: String,
}

fn address_score(octets: [u8; 4]) -> u8 {
    if octets[0] == 192 && octets[1] == 168 {
        return 0;
    }
    if octets[0] == 10 {
        return 1;
    }
    if octets[0] == 172 && (16..=31).contains(&octets[1]) {
        return 2;
    }
    5
}

/// Private RFC1918 address usable from a client on the same LAN.
fn is_private_lan_ip(ip: &Ipv4Addr) -> bool {
    let o = ip.octets();
    if o[0] == 10 {
        return true;
    }
    if o[0] == 172 && (16..=31).contains(&o[1]) {
        return true;
    }
    if o[0] == 192 && o[1] == 168 {
        return true;
    }
    false
}

fn is_virtual_interface_name(name: &str) -> bool {
    let n = name.to_lowercase();
    const PREFIXES: &[&str] = &[
        "lo",
        "utun",
        "bridge",
        "awdl",
        "llw",
        "anpi",
        "gif",
        "stf",
        "vmenet",
        "vboxnet",
        "vmnet",
        "docker",
        "veth",
        "br-",
        "vethernet",
        "virtualbox",
        "vmware",
        "hyper-v",
        "npcap",
        "tailscale",
        "wireguard",
        "wintun",
        "loopback",
        "isatap",
        "teredo",
    ];
    PREFIXES.iter().any(|prefix| n.starts_with(prefix))
}

pub fn list_lan_addresses() -> Vec<String> {
    let mut addresses = Vec::new();
    let Ok(ifaces) = get_if_addrs() else {
        return addresses;
    };

    for iface in ifaces {
        if iface.is_loopback() || is_virtual_interface_name(&iface.name) {
            continue;
        }
        if let IfAddr::V4(v4) = iface.addr {
            let ip = v4.ip;
            if is_private_lan_ip(&ip) {
                addresses.push(ip.to_string());
            }
        }
    }

    addresses.sort_by_key(|a| {
        a.parse::<Ipv4Addr>()
            .map(|ip| address_score(ip.octets()))
            .unwrap_or(9)
    });
    addresses.dedup();
    addresses
}

#[cfg(target_os = "macos")]
fn resolve_host_name() -> Option<String> {
    let output = Command::new("scutil")
        .args(["--get", "LocalHostName"])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(format!("{name}.local"))
}

#[cfg(windows)]
fn resolve_host_name() -> Option<String> {
    std::env::var("COMPUTERNAME")
        .ok()
        .filter(|s| !s.trim().is_empty())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn resolve_host_name() -> Option<String> {
    let output = Command::new("hostname").output().ok()?;
    if !output.status.success() {
        return None;
    }
    let name = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if name.is_empty() {
        return None;
    }
    Some(name)
}

pub fn get_network_info() -> NetworkInfo {
    NetworkInfo {
        addresses: list_lan_addresses(),
        host_name: resolve_host_name(),
        localhost: "127.0.0.1".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_lan_ip_detection() {
        assert!(is_private_lan_ip(&"192.168.2.100".parse().unwrap()));
        assert!(is_private_lan_ip(&"10.0.0.200".parse().unwrap()));
        assert!(!is_private_lan_ip(&"169.254.190.112".parse().unwrap()));
        assert!(!is_private_lan_ip(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn virtual_interface_names() {
        assert!(is_virtual_interface_name("bridge100"));
        assert!(is_virtual_interface_name("utun4"));
        assert!(is_virtual_interface_name("vEthernet (WSL)"));
        assert!(!is_virtual_interface_name("en0"));
        assert!(!is_virtual_interface_name("Wi-Fi"));
        assert!(!is_virtual_interface_name("Ethernet"));
    }

    #[test]
    fn address_score_prefers_home_lan() {
        assert!(address_score([192, 168, 1, 2]) < address_score([10, 0, 0, 1]));
        assert!(address_score([10, 0, 0, 1]) < address_score([172, 16, 0, 1]));
        assert!(address_score([172, 16, 0, 1]) < address_score([8, 8, 8, 8]));
    }
}
