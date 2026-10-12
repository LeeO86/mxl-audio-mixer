//! A minimal RTSP publisher (RFC 2326: OPTIONS, ANNOUNCE, SETUP, RECORD) sending
//! Opus (RFC 7587) as RTP interleaved on the RTSP TCP connection, which is how
//! MediaMTX takes a published stream.

use std::io::{self, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// RTP payload type of the Opus track.
pub const PAYLOAD_TYPE: u8 = 96;

/// `rtsp://host[:port]/path` → (host, port, path).
pub fn parse_url(url: &str) -> Option<(String, u16, String)> {
    let rest = url.strip_prefix("rtsp://")?;
    let (authority, path) = rest.split_once('/').unwrap_or((rest, ""));
    let (host, port) = match authority.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse().ok()?),
        None => (authority.to_string(), 554),
    };
    (!host.is_empty()).then(|| (host, port, format!("/{path}")))
}

/// The SDP of a stereo Opus track.
pub fn sdp(name: &str) -> String {
    format!(
        "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns={name}\r\nc=IN IP4 0.0.0.0\r\nt=0 0\r\n\
         m=audio 0 RTP/AVP {PAYLOAD_TYPE}\r\na=rtpmap:{PAYLOAD_TYPE} opus/48000/2\r\n\
         a=fmtp:{PAYLOAD_TYPE} sprop-stereo=1\r\na=control:trackID=0\r\n"
    )
}

/// An RTP packet (version 2, no padding, extension or CSRC).
pub fn rtp(seq: u16, timestamp: u32, ssrc: u32, payload: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(12 + payload.len());
    p.extend_from_slice(&[0x80, PAYLOAD_TYPE]);
    p.extend_from_slice(&seq.to_be_bytes());
    p.extend_from_slice(&timestamp.to_be_bytes());
    p.extend_from_slice(&ssrc.to_be_bytes());
    p.extend_from_slice(payload);
    p
}

pub struct Publisher {
    stream: TcpStream,
    seq: u16,
    ssrc: u32,
}

impl Publisher {
    /// Connect to `url` and start recording a stereo Opus track.
    pub fn connect(url: &str, timeout: Duration) -> io::Result<Self> {
        let bad = |m: &str| io::Error::new(io::ErrorKind::InvalidInput, m.to_string());
        let (host, port, _) = parse_url(url).ok_or_else(|| bad("not an rtsp:// URL"))?;
        let addr = (host.as_str(), port)
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| bad("no address"))?;
        let stream = TcpStream::connect_timeout(&addr, timeout)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        let mut session = Session {
            stream,
            cseq: 0,
            url: url.to_string(),
        };
        session.request("OPTIONS", url, &[], None)?;
        let name = url.rsplit('/').next().unwrap_or("stream");
        session.request(
            "ANNOUNCE",
            url,
            &[("Content-Type", "application/sdp")],
            Some(&sdp(name)),
        )?;
        let track = format!("{url}/trackID=0");
        let headers = session.request(
            "SETUP",
            &track,
            &[(
                "Transport",
                "RTP/AVP/TCP;unicast;interleaved=0-1;mode=record",
            )],
            None,
        )?;
        let id = headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("session"))
            .map(|(_, v)| v.split(';').next().unwrap_or("").trim().to_string())
            .ok_or_else(|| io::Error::other("SETUP gave no session"))?;
        session.request(
            "RECORD",
            url,
            &[("Session", &id), ("Range", "npt=0.000-")],
            None,
        )?;
        let Session { stream, .. } = session;
        // MediaMTX sends RTCP receiver reports on the connection; read and drop
        // them so that its buffer never fills.
        let mut drain = stream.try_clone()?;
        drain.set_read_timeout(None)?;
        std::thread::Builder::new()
            .name("rtsp-drain".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                while matches!(drain.read(&mut buf), Ok(n) if n > 0) {}
            })?;
        let ssrc = rand_u32();
        Ok(Self {
            stream,
            seq: (ssrc >> 16) as u16,
            ssrc,
        })
    }

    /// Send one Opus packet with its RTP timestamp (48 kHz samples).
    pub fn send(&mut self, opus: &[u8], timestamp: u32) -> io::Result<()> {
        let packet = rtp(self.seq, timestamp, self.ssrc, opus);
        self.seq = self.seq.wrapping_add(1);
        let mut frame = Vec::with_capacity(4 + packet.len());
        frame.extend_from_slice(&[b'$', 0]);
        frame.extend_from_slice(&(packet.len() as u16).to_be_bytes());
        frame.extend_from_slice(&packet);
        self.stream.write_all(&frame)
    }
}

impl Drop for Publisher {
    fn drop(&mut self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}

fn rand_u32() -> u32 {
    let mut b = [0u8; 4];
    if std::fs::File::open("/dev/urandom")
        .and_then(|mut f| f.read_exact(&mut b))
        .is_err()
    {
        b = (std::process::id() ^ 0x5eed_1234).to_le_bytes();
    }
    u32::from_le_bytes(b)
}

struct Session {
    stream: TcpStream,
    cseq: u32,
    url: String,
}

impl Session {
    /// Send a request and read its response; an answer other than 2xx is an error.
    fn request(
        &mut self,
        method: &str,
        url: &str,
        headers: &[(&str, &str)],
        body: Option<&str>,
    ) -> io::Result<Vec<(String, String)>> {
        self.cseq += 1;
        let mut req = format!(
            "{method} {url} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: mxl-audio-mixer\r\n",
            self.cseq
        );
        for (k, v) in headers {
            req.push_str(&format!("{k}: {v}\r\n"));
        }
        if let Some(b) = body {
            req.push_str(&format!("Content-Length: {}\r\n", b.len()));
        }
        req.push_str("\r\n");
        if let Some(b) = body {
            req.push_str(b);
        }
        self.stream.write_all(req.as_bytes())?;
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        while !head.ends_with(b"\r\n\r\n") {
            if self.stream.read(&mut byte)? == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "RTSP connection closed",
                ));
            }
            head.push(byte[0]);
            if head.len() > 16 * 1024 {
                return Err(io::Error::other("RTSP response too long"));
            }
        }
        let text = String::from_utf8_lossy(&head);
        let mut lines = text.split("\r\n");
        let status = lines.next().unwrap_or("");
        let code: u16 = status
            .split(' ')
            .nth(1)
            .and_then(|c| c.parse().ok())
            .unwrap_or(0);
        let fields: Vec<(String, String)> = lines
            .filter_map(|l| l.split_once(':'))
            .map(|(k, v)| (k.trim().to_string(), v.trim().to_string()))
            .collect();
        let length: usize = fields
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, v)| v.parse().ok())
            .unwrap_or(0);
        let mut rest = vec![0u8; length];
        self.stream.read_exact(&mut rest)?;
        if !(200..300).contains(&code) {
            return Err(io::Error::other(format!("{method} {}: {status}", self.url)));
        }
        Ok(fields)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_sdp_and_rtp() {
        assert_eq!(
            parse_url("rtsp://127.0.0.1:8954/mxl-audio-mixer/main"),
            Some(("127.0.0.1".into(), 8954, "/mxl-audio-mixer/main".into()))
        );
        assert_eq!(
            parse_url("rtsp://mediamtx/p"),
            Some(("mediamtx".into(), 554, "/p".into()))
        );
        assert_eq!(parse_url("http://x/y"), None);
        assert!(sdp("main").contains("a=rtpmap:96 opus/48000/2\r\n"));
        let p = rtp(0x1234, 960, 0xdead_beef, &[1, 2, 3]);
        assert_eq!(
            p,
            vec![0x80, 96, 0x12, 0x34, 0, 0, 3, 0xc0, 0xde, 0xad, 0xbe, 0xef, 1, 2, 3]
        );
    }
}
