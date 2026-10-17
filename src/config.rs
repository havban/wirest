//! WireGuard config parsing, key handling and on-disk tunnel storage.

use base64::{engine::general_purpose::STANDARD, Engine};
use std::fs::{self, OpenOptions, Permissions};
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

#[derive(Default, Clone)]
pub struct Interface {
    pub private_key: Option<String>,
    pub addresses: Vec<String>,
    pub dns: Vec<String>,
    pub listen_port: Option<String>,
    pub mtu: Option<String>,
}

#[derive(Default, Clone)]
pub struct Peer {
    pub public_key: Option<String>,
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<String>,
    pub keepalive: Option<String>,
    pub has_psk: bool,
}

#[derive(Default, Clone)]
pub struct Parsed {
    pub interface: Interface,
    pub peers: Vec<Peer>,
    pub errors: Vec<String>,
}

enum Section {
    None,
    Interface,
    Peer,
}

pub fn parse(text: &str) -> Parsed {
    let mut p = Parsed::default();
    let mut section = Section::None;
    let mut saw_interface = false;

    for (n, raw) in text.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let ln = n + 1;

        if line.starts_with('[') {
            match line.to_ascii_lowercase().as_str() {
                "[interface]" => {
                    if saw_interface {
                        p.errors.push(format!("Line {ln}: duplicate [Interface] section"));
                    }
                    saw_interface = true;
                    section = Section::Interface;
                }
                "[peer]" => {
                    p.peers.push(Peer::default());
                    section = Section::Peer;
                }
                other => {
                    p.errors.push(format!("Line {ln}: unknown section {other}"));
                    section = Section::None;
                }
            }
            continue;
        }

        let Some((key, value)) = line.split_once('=') else {
            p.errors.push(format!("Line {ln}: expected `Key = Value`"));
            continue;
        };
        let key = key.trim().to_ascii_lowercase();
        let value = value.trim().to_string();

        match section {
            Section::Interface => {
                let iface = &mut p.interface;
                match key.as_str() {
                    "privatekey" => {
                        if !valid_key(&value) {
                            p.errors.push(format!("Line {ln}: PrivateKey is not a valid key"));
                        }
                        iface.private_key = Some(value);
                    }
                    "address" => iface.addresses.extend(split_list(&value)),
                    "dns" => iface.dns.extend(split_list(&value)),
                    "listenport" => iface.listen_port = Some(value),
                    "mtu" => iface.mtu = Some(value),
                    "table" | "fwmark" | "saveconfig" | "preup" | "postup" | "predown"
                    | "postdown" => {}
                    _ => p.errors.push(format!("Line {ln}: unknown [Interface] key `{key}`")),
                }
            }
            Section::Peer => {
                let peer = p.peers.last_mut().expect("peer section pushed a peer");
                match key.as_str() {
                    "publickey" => {
                        if !valid_key(&value) {
                            p.errors.push(format!("Line {ln}: PublicKey is not a valid key"));
                        }
                        peer.public_key = Some(value);
                    }
                    "presharedkey" => {
                        if !valid_key(&value) {
                            p.errors.push(format!("Line {ln}: PresharedKey is not a valid key"));
                        }
                        peer.has_psk = true;
                    }
                    "endpoint" => peer.endpoint = Some(value),
                    "allowedips" => peer.allowed_ips.extend(split_list(&value)),
                    "persistentkeepalive" => peer.keepalive = Some(value),
                    _ => p.errors.push(format!("Line {ln}: unknown [Peer] key `{key}`")),
                }
            }
            Section::None => p.errors.push(format!("Line {ln}: key outside of a section")),
        }
    }

    if !saw_interface {
        p.errors.push("Missing [Interface] section".into());
    } else if p.interface.private_key.is_none() {
        p.errors.push("[Interface] is missing PrivateKey".into());
    }
    for (i, peer) in p.peers.iter().enumerate() {
        if peer.public_key.is_none() {
            p.errors.push(format!("Peer {} is missing PublicKey", i + 1));
        }
    }
    p
}

fn split_list(v: &str) -> Vec<String> {
    v.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn decode_key(s: &str) -> Option<[u8; 32]> {
    STANDARD.decode(s.trim()).ok()?.try_into().ok()
}

pub fn valid_key(s: &str) -> bool {
    decode_key(s).is_some()
}

/// Same as `wg genkey`: 32 random bytes, clamped for Curve25519.
pub fn generate_private_key() -> String {
    let mut b = [0u8; 32];
    getrandom::getrandom(&mut b).expect("system RNG unavailable");
    b[0] &= 248;
    b[31] &= 127;
    b[31] |= 64;
    STANDARD.encode(b)
}

/// Same as `wg pubkey`.
pub fn public_key(private_key: &str) -> Option<String> {
    let secret = x25519_dalek::StaticSecret::from(decode_key(private_key)?);
    Some(STANDARD.encode(x25519_dalek::PublicKey::from(&secret).as_bytes()))
}

pub fn template(private_key: &str) -> String {
    format!(
        "[Interface]\n\
         PrivateKey = {private_key}\n\
         Address = 10.0.0.2/32\n\
         DNS = 1.1.1.1\n\
         \n\
         [Peer]\n\
         # Fill in your server's public key and endpoint\n\
         PublicKey = \n\
         AllowedIPs = 0.0.0.0/0, ::/0\n\
         Endpoint = vpn.example.com:51820\n\
         PersistentKeepalive = 25\n"
    )
}

/// wg-quick derives the interface name from the file name, so it must be a
/// valid interface name: 1-15 chars of `[a-zA-Z0-9_=+.-]`.
pub fn valid_name(name: &str) -> bool {
    (1..=15).contains(&name.len()) && name.chars().all(name_char)
}

fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || "_=+.-".contains(c)
}

pub fn sanitize_name(raw: &str) -> String {
    let s: String = raw
        .chars()
        .map(|c| if name_char(c) { c } else { '_' })
        .take(15)
        .collect();
    if s.is_empty() {
        "tunnel".into()
    } else {
        s
    }
}

pub fn tunnels_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Wirus")
        .join("tunnels")
}

pub fn ensure_dir(dir: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, Permissions::from_mode(0o700))
}

/// Configs contain private keys, so keep them owner-only.
pub fn write_secure(path: &Path, text: &str) -> std::io::Result<()> {
    let mut f = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(text.as_bytes())?;
    fs::set_permissions(path, Permissions::from_mode(0o600))
}

pub fn list_configs(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "conf"))
        .collect();
    v.sort_by_key(|p| p.file_name().map(|n| n.to_ascii_lowercase()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keypair_roundtrip() {
        let k = generate_private_key();
        assert!(valid_key(&k));
        assert!(valid_key(&public_key(&k).unwrap()));
    }

    #[test]
    fn known_public_key() {
        // RFC 7748 section 6.1 test vector (Alice).
        let private = STANDARD.encode(
            [0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2,
             0x66, 0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5,
             0x1d, 0xb9, 0x2c, 0x2a],
        );
        let public = STANDARD.encode(
            [0x85, 0x20, 0xf0, 0x09, 0x89, 0x30, 0xa7, 0x54, 0x74, 0x8b, 0x7d, 0xdc, 0xb4, 0x3e,
             0xf7, 0x5a, 0x0d, 0xbf, 0x3a, 0x0d, 0x26, 0x38, 0x1a, 0xf4, 0xeb, 0xa4, 0xa9, 0x8e,
             0xaa, 0x9b, 0x4e, 0x6a],
        );
        assert_eq!(public_key(&private).unwrap(), public);
    }

    #[test]
    fn parses_config() {
        let k = generate_private_key();
        let pk = public_key(&k).unwrap();
        let text = format!(
            "[Interface]\nPrivateKey = {k}\nAddress = 10.0.0.2/32, fd00::2/128\nDNS = 1.1.1.1\n\
             # comment\n[Peer]\nPublicKey = {pk}\nAllowedIPs = 0.0.0.0/0\nEndpoint = a.b:51820\n"
        );
        let p = parse(&text);
        assert!(p.errors.is_empty(), "{:?}", p.errors);
        assert_eq!(p.interface.addresses.len(), 2);
        assert_eq!(p.peers.len(), 1);
        assert_eq!(p.peers[0].endpoint.as_deref(), Some("a.b:51820"));
    }

    #[test]
    fn reports_errors() {
        let p = parse("[Interface]\nAddress = 10.0.0.2/32\nBogus\n[Peer]\nEndpoint = x:1\n");
        assert_eq!(p.errors.len(), 3, "{:?}", p.errors);
    }

    #[test]
    fn names() {
        assert!(valid_name("wg0"));
        assert!(!valid_name("this-name-is-too-long"));
        assert!(!valid_name("has space"));
        assert_eq!(sanitize_name("my vpn (home)"), "my_vpn__home_");
    }
}
