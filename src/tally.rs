//! Tally (§10.3): a TSL UMD 5.0 receiver on UDP and TCP (DLE/STX framing), as in
//! mxl-multiviewer and mxl-webrtc-monitor. Display messages of `TSL_SCREEN` (or
//! every screen) set the lamps of a tally index (`TSL_MAP`, or the display index);
//! strips map to a tally index and follow it with audio-follows-video.
//!
//! Lamp values: 0 off, 1 red, 2 green, 3 amber. LH (left) lit is on air, RH
//! (right) lit is on preview.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Serialize;

use crate::config::Config;

const DLE: u8 = 0xfe;
const STX: u8 = 0x02;
const ETX: u8 = 0x03;
/// Most TCP senders at once.
const MAX_TCP_CLIENTS: usize = 8;

/// One display message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Display {
    pub screen: u16,
    pub index: u16,
    pub rh: u8,
    pub text: u8,
    pub lh: u8,
    pub brightness: u8,
    pub label: String,
}

fn read16(p: &[u8]) -> u16 {
    u16::from_le_bytes([p[0], p[1]])
}

/// Splits a byte stream into TSL packets: DLE/STX starts a packet, DLE DLE is one
/// 0xFE byte, and a packet ends at DLE/ETX, at the next DLE/STX, or when it holds
/// PBC + 2 bytes.
#[derive(Default)]
pub struct Framer {
    buffer: Vec<u8>,
    in_packet: bool,
    dle: bool,
}

impl Framer {
    pub fn feed(&mut self, data: &[u8]) -> Vec<Vec<u8>> {
        let mut packets = Vec::new();
        for &byte in data {
            if !self.dle && byte == DLE {
                self.dle = true;
                continue;
            }
            let escaped = self.dle;
            self.dle = false;
            if escaped && byte != DLE {
                if byte == STX || byte == ETX {
                    if self.in_packet && !self.buffer.is_empty() {
                        packets.push(std::mem::take(&mut self.buffer));
                    }
                    self.buffer.clear();
                    self.in_packet = byte == STX;
                }
                continue;
            }
            if !self.in_packet {
                continue;
            }
            self.buffer.push(byte);
            if self.buffer.len() >= 2 && self.buffer.len() == read16(&self.buffer) as usize + 2 {
                packets.push(std::mem::take(&mut self.buffer));
                self.in_packet = false;
            }
        }
        packets
    }
}

/// The packets of one UDP datagram: the bare packet, or the packets of a DLE/STX
/// wrapped datagram.
pub fn datagram(data: &[u8]) -> Vec<Vec<u8>> {
    if data.len() >= 2 && data[0] == DLE && data[1] == STX {
        Framer::default().feed(data)
    } else {
        vec![data.to_vec()]
    }
}

/// Parse one TSL 5.0 packet (PBC first). Broadcast and control messages carry no
/// lamps and give an empty list.
pub fn parse(body: &[u8]) -> Result<Vec<Display>, &'static str> {
    if body.len() < 6 {
        return Err("short");
    }
    let end = read16(body) as usize + 2;
    if end > body.len() {
        return Err("pbc");
    }
    let unicode = body[3] & 0x01 != 0;
    if body[3] & 0x02 != 0 {
        return Ok(Vec::new());
    }
    let screen = read16(&body[4..]);
    let mut out = Vec::new();
    let mut cursor = 6;
    while cursor + 4 <= end {
        let index = read16(&body[cursor..]);
        let control = read16(&body[cursor + 2..]);
        cursor += 4;
        if cursor + 2 > end {
            return Err("length");
        }
        let length = read16(&body[cursor..]) as usize;
        cursor += 2;
        if cursor + length > end {
            return Err("text");
        }
        let text = &body[cursor..cursor + length];
        cursor += length;
        // A control message (bit 15) carries data, not a label.
        if control & 0x8000 != 0 {
            continue;
        }
        let label = if unicode {
            let units: Vec<u16> = text.chunks_exact(2).map(read16).collect();
            char::decode_utf16(units)
                .map(|c| c.unwrap_or(char::REPLACEMENT_CHARACTER))
                .filter(|c| *c != '\0')
                .collect()
        } else {
            text.iter()
                .map(|&b| {
                    if b < 0x80 {
                        b as char
                    } else {
                        char::REPLACEMENT_CHARACTER
                    }
                })
                .collect::<String>()
                .trim_end_matches('\0')
                .to_string()
        };
        out.push(Display {
            screen,
            index,
            rh: (control & 0x3) as u8,
            text: ((control >> 2) & 0x3) as u8,
            lh: ((control >> 4) & 0x3) as u8,
            brightness: ((control >> 6) & 0x3) as u8,
            label,
        });
    }
    Ok(out)
}

/// The lamps of one tally index.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Lamps {
    pub screen: u16,
    pub display: u16,
    pub lh: u8,
    pub rh: u8,
    pub text: u8,
    pub label: String,
    /// Seconds since the last message for it.
    pub age_s: f64,
}

#[derive(Default)]
pub struct Tally {
    pub packets: u64,
    pub errors: u64,
    last: Option<Instant>,
    lamps: BTreeMap<u16, (Lamps, Instant)>,
}

impl Tally {
    /// Apply one packet with `TSL_SCREEN` and `TSL_MAP`; true when a lamp changed.
    pub fn packet(&mut self, config: &Config, packet: &[u8]) -> bool {
        self.packets += 1;
        self.last = Some(Instant::now());
        let displays = match parse(packet) {
            Ok(d) => d,
            Err(_) => {
                self.errors += 1;
                return false;
            }
        };
        let mut changed = false;
        for d in displays {
            if config.tsl_screen.is_some_and(|s| s != d.screen) {
                continue;
            }
            let index = if config.tsl_map.is_empty() {
                Some(d.index)
            } else {
                config.tsl_map.get(&d.index).copied()
            };
            let Some(index) = index else { continue };
            let lamps = Lamps {
                screen: d.screen,
                display: d.index,
                lh: d.lh,
                rh: d.rh,
                text: d.text,
                label: d.label,
                age_s: 0.0,
            };
            let before = self.lamps.get(&index).map(|(l, _)| (l.lh, l.rh));
            changed |= before != Some((lamps.lh, lamps.rh));
            self.lamps.insert(index, (lamps, Instant::now()));
        }
        changed
    }

    pub fn on_air(&self, index: u16) -> bool {
        self.lamps.get(&index).is_some_and(|(l, _)| l.lh != 0)
    }

    /// Tally indexes whose left lamp is lit.
    pub fn on_air_indexes(&self) -> BTreeSet<u16> {
        self.lamps
            .iter()
            .filter(|(_, (l, _))| l.lh != 0)
            .map(|(i, _)| *i)
            .collect()
    }

    pub fn preview(&self, index: u16) -> bool {
        self.lamps.get(&index).is_some_and(|(l, _)| l.rh != 0)
    }

    pub fn lamps(&self) -> BTreeMap<u16, Lamps> {
        self.lamps
            .iter()
            .map(|(i, (l, at))| {
                let mut l = l.clone();
                l.age_s = (at.elapsed().as_secs_f64() * 10.0).round() / 10.0;
                (*i, l)
            })
            .collect()
    }

    pub fn last_packet_age_s(&self) -> Option<f64> {
        self.last.map(|t| t.elapsed().as_secs_f64())
    }
}

/// The bound TSL sockets.
pub struct Sockets {
    udp: tokio::net::UdpSocket,
    tcp: tokio::net::TcpListener,
}

pub async fn bind(config: &Config) -> std::io::Result<Sockets> {
    let udp = tokio::net::UdpSocket::bind(("0.0.0.0", config.tsl_udp_port)).await?;
    let tcp = tokio::net::TcpListener::bind(("0.0.0.0", config.tsl_tcp_port)).await?;
    Ok(Sockets { udp, tcp })
}

/// What receives every packet.
pub type Apply = Arc<dyn Fn(&[u8]) + Send + Sync>;

/// Receive on both sockets; `apply` gets every packet.
pub fn spawn(sockets: Sockets, apply: Apply) {
    let Sockets { udp, tcp } = sockets;
    let on_udp = apply.clone();
    tokio::spawn(async move {
        let mut buf = vec![0u8; 65536];
        loop {
            match udp.recv(&mut buf).await {
                Ok(n) => {
                    for p in datagram(&buf[..n]) {
                        on_udp(&p);
                    }
                }
                Err(e) => tracing::warn!("TSL UDP: {e}"),
            }
        }
    });
    let clients = Arc::new(Mutex::new(0usize));
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, peer)) = tcp.accept().await else {
                continue;
            };
            {
                let mut n = clients.lock().unwrap_or_else(|e| e.into_inner());
                if *n >= MAX_TCP_CLIENTS {
                    tracing::warn!("TSL TCP: {peer} refused, {MAX_TCP_CLIENTS} senders connected");
                    continue;
                }
                *n += 1;
            }
            let apply = apply.clone();
            let clients = clients.clone();
            tokio::spawn(async move {
                use tokio::io::AsyncReadExt;
                let mut framer = Framer::default();
                let mut buf = vec![0u8; 65536];
                while let Ok(n) = stream.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    for p in framer.feed(&buf[..n]) {
                        apply(&p);
                    }
                }
                *clients.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
            });
        }
    });
}

/// A TSL 5.0 packet (PBC first) with one display message per entry, ASCII labels;
/// for tests and the test sender.
pub fn encode(screen: u16, displays: &[(u16, u8, u8, &str)]) -> Vec<u8> {
    let mut body = vec![0u8, 0, 0, 0];
    body.extend_from_slice(&screen.to_le_bytes());
    for &(index, lh, rh, label) in displays {
        body.extend_from_slice(&index.to_le_bytes());
        let control = u16::from(rh & 3) | (u16::from(lh & 3) << 4) | (3 << 6);
        body.extend_from_slice(&control.to_le_bytes());
        body.extend_from_slice(&(label.len() as u16).to_le_bytes());
        body.extend_from_slice(label.as_bytes());
    }
    let pbc = (body.len() - 2) as u16;
    body[..2].copy_from_slice(&pbc.to_le_bytes());
    body
}

/// `packet` wrapped for TCP: DLE/STX, the packet with DLE doubled.
pub fn wrap(packet: &[u8]) -> Vec<u8> {
    let mut out = vec![DLE, STX];
    for &b in packet {
        out.push(b);
        if b == DLE {
            out.push(DLE);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> Config {
        let env = vars
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        crate::config::load(&env).unwrap()
    }

    #[test]
    fn packets_round_trip_and_unicode_labels() {
        let p = encode(2, &[(5, 1, 0, "Cam 1"), (6, 0, 2, "Cam 2")]);
        let d = parse(&p).unwrap();
        assert_eq!(d.len(), 2);
        assert_eq!(
            (
                d[0].screen,
                d[0].index,
                d[0].lh,
                d[0].rh,
                d[0].label.as_str()
            ),
            (2, 5, 1, 0, "Cam 1")
        );
        assert_eq!(
            (d[1].index, d[1].lh, d[1].rh, d[1].brightness),
            (6, 0, 2, 3)
        );

        // UTF-16 label "Zürich" with the unicode flag.
        let label: Vec<u8> = "Zürich"
            .encode_utf16()
            .flat_map(|u| u.to_le_bytes())
            .collect();
        let mut body = vec![0, 0, 0, 1, 0, 0, 9, 0, 0x10, 0];
        body.extend_from_slice(&(label.len() as u16).to_le_bytes());
        body.extend_from_slice(&label);
        let pbc = (body.len() - 2) as u16;
        body[..2].copy_from_slice(&pbc.to_le_bytes());
        let d = parse(&body).unwrap();
        assert_eq!((d[0].index, d[0].lh, d[0].label.as_str()), (9, 1, "Zürich"));
        assert!(parse(&body[..body.len() - 1]).is_err());
    }

    #[test]
    fn framing_with_escapes_split_across_reads() {
        // Index 0xFE puts a DLE byte into the packet: sent as DLE DLE.
        let a = encode(0, &[(0xfe, 1, 0, "A")]);
        let b = encode(0, &[(3, 0, 1, "B")]);
        let mut stream = wrap(&a);
        stream.extend_from_slice(&wrap(&b));
        let mut f = Framer::default();
        let mut got = Vec::new();
        for chunk in stream.chunks(3) {
            got.extend(f.feed(chunk));
        }
        assert_eq!(got, vec![a.clone(), b]);
        assert_eq!(datagram(&wrap(&a)), vec![a.clone()]);
        assert_eq!(datagram(&a), vec![a]);
    }

    #[test]
    fn screen_and_map() {
        let mut t = Tally::default();
        let c = config(&[("TSL_SCREEN", "1"), ("TSL_MAP", "4:10")]);
        assert!(!t.packet(&c, &encode(0, &[(4, 1, 0, "other screen")])));
        assert!(t.packet(&c, &encode(1, &[(4, 1, 0, "Cam"), (5, 1, 0, "unmapped")])));
        assert!(t.on_air(10) && !t.on_air(4) && !t.on_air(5));
        assert!(!t.packet(&c, &encode(1, &[(4, 1, 0, "Cam")])), "same state");
        assert!(t.packet(&c, &encode(1, &[(4, 0, 1, "Cam")])));
        assert!(!t.on_air(10) && t.preview(10));
        assert!(!t.packet(&c, &[1, 2, 3]));
        assert_eq!((t.packets, t.errors), (5, 1));
        let plain = config(&[]);
        let mut t = Tally::default();
        t.packet(&plain, &encode(7, &[(4, 2, 0, "")]));
        assert!(t.on_air(4));
    }
}
