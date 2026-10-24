//! Software updater on top of winget (Windows Package Manager). Listing parses the table
//! `winget upgrade` prints; updating goes through the fixes runner with ids `winget:<PackageId>`
//! so the output streams live. Package ids are checked against a strict character set and
//! passed as separate arguments, never through a shell.

use crate::ps;
use serde::Serialize;
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::{Duration, Instant};

#[derive(Serialize, Clone, Debug)]
pub struct Upd {
    pub name: String,
    pub id: String,
    pub version: String,
    pub available: String,
    pub source: String,
}

#[derive(Serialize)]
pub struct Listing {
    pub items: Vec<Upd>,
    /// Something the updater changed on the way, shown above the list.
    pub note: Option<String>,
}

pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 120 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-')) && !id.starts_with('-')
}

fn clean_line(l: &str) -> String {
    let t = l.trim_end_matches('\r');
    t.rsplit('\r').next().unwrap_or("").to_string()
}

pub fn parse(text: &str) -> Vec<Upd> {
    let lines: Vec<String> = text.split('\n').map(clean_line).collect();
    let sep = match lines.iter().position(|l| l.len() >= 10 && l.chars().all(|c| c == '-')) {
        Some(i) if i > 0 => i,
        _ => return vec![],
    };
    let header = &lines[sep - 1];
    let col = |n: &str| header.find(n);
    let (id_p, ver_p, av_p) = match (col("Id"), col("Version"), col("Available")) {
        (Some(a), Some(b), Some(c)) if a < b && b < c => (a, b, c),
        _ => return vec![],
    };
    let src_p = col("Source");
    let mut out = Vec::new();
    for l in &lines[sep + 1..] {
        if l.trim().is_empty() {
            break;
        }
        let ch: Vec<char> = l.chars().collect();
        if ch.len() <= av_p {
            continue;
        }
        let cut = |a: usize, b: Option<usize>| -> String {
            let b = b.unwrap_or(ch.len()).min(ch.len());
            if a >= b { String::new() } else { ch[a..b].iter().collect::<String>().trim().to_string() }
        };
        let id = cut(id_p, Some(ver_p));
        if !valid_id(&id) {
            continue;
        }
        out.push(Upd { name: cut(0, Some(id_p)), id, version: cut(ver_p, Some(av_p)), available: cut(av_p, src_p), source: src_p.map(|p| cut(p, None)).unwrap_or_default() });
    }
    out
}

const CDN: &str = "cdn.winget.microsoft.com";

/// The CDN's addresses in the order Windows returns them, which is the order winget tries them in.
fn cdn_addrs() -> Vec<SocketAddr> {
    (CDN, 80).to_socket_addrs().map(|i| i.collect()).unwrap_or_default()
}

/// Asks the CDN for a plain-HTTP answer large enough to span many full-size packets.
/// Some(true): the data arrived (or a complete short answer did). Some(false): the connection
/// opened but the data stalled. None: no connection at all.
fn delivers(addr: SocketAddr) -> Option<bool> {
    use std::io::{Read, Write};
    let mut s = TcpStream::connect_timeout(&addr, Duration::from_secs(4)).ok()?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let req = format!("GET /cache/source.msix HTTP/1.1
Host: {}
Range: bytes=0-65535
Connection: close

", CDN);
    s.write_all(req.as_bytes()).ok()?;
    let (mut buf, mut got, end) = ([0u8; 16384], 0usize, Instant::now() + Duration::from_secs(6));
    loop {
        match s.read(&mut buf) {
            Ok(0) => return Some(true),
            Ok(n) => got += n,
            Err(_) => return Some(got >= 32768),
        }
        if got >= 32768 || Instant::now() > end {
            return Some(got >= 32768);
        }
    }
}

/// Is Windows going to send winget over a broken IPv6 route? Some internet providers have an
/// IPv6 path where the connection opens and small packets pass, but full-size ones are dropped,
/// so TLS and downloads stall and winget waits forever. A connect test cannot see that, so this
/// pulls real data over both families. Only matters when IPv6 comes first in the order Windows
/// returns the addresses, which is the order winget tries them in.
fn ipv6_blackhole() -> bool {
    let addrs = cdn_addrs();
    let (Some(v6), Some(v4)) = (addrs.iter().find(|a| a.is_ipv6()).copied(), addrs.iter().find(|a| a.is_ipv4()).copied()) else {
        return false;
    };
    if addrs[0].is_ipv4() {
        return false;
    }
    let four = std::thread::spawn(move || delivers(v4));
    let six = delivers(v6);
    six != Some(true) && four.join().ok().flatten() == Some(true)
}

/// Error texts starting with `IPV6:` or `TIMEOUT:` are recognised by the UI. A broken IPv6 route
/// is repaired on the spot when the app runs as Administrator, so the list just loads.
pub fn list() -> Result<Listing, String> {
    let mut note = None;
    if ipv6_blackhole() {
        let fixed = crate::util::is_admin() && crate::fixes::prefer_ipv4_quietly() && cdn_addrs().first().is_some_and(|a| a.is_ipv4());
        if !fixed {
            return Err("IPV6:winget cannot reach its server from this PC. Microsoft's package server (cdn.winget.microsoft.com) answers over IPv4, but over IPv6 the data never arrives, and Windows is trying IPv6 first, so winget would wait forever. \"Prefer IPv4\" makes Windows try IPv4 first. It needs Administrator: run the app as Administrator and this is fixed automatically.".into());
        }
        note = Some("Your internet connection's IPv6 route to Microsoft's servers is broken, so Windows was set to try IPv4 first. IPv6 stays switched on, and you can undo this any time in Troubleshoot.".to_string());
    }
    let o = ps::run_capture("winget", &["upgrade", "--source", "winget", "--accept-source-agreements", "--disable-interactivity"], &[], 60)
        .map_err(|_| "winget (Windows Package Manager) was not found. Install \"App Installer\" from the Microsoft Store.".to_string())?;
    if o.timed_out {
        return Err("TIMEOUT:winget did not answer within 60 seconds, so the check was stopped. Its server is reachable from this PC, so winget may be refreshing a large package list on a slow connection, or Microsoft's servers are slow right now. Try again in a few minutes.".into());
    }
    Ok(Listing { items: parse(&o.stdout), note })
}
