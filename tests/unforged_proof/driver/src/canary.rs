//! Loopback-only synthetic network controls, not provider/adapter clients.
//! No DNS, non-loopback address, secret, or external service is accepted.
use std::fs;
use std::io::{Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpListener, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

fn addr(port: u16) -> SocketAddr {
    SocketAddrV4::new(Ipv4Addr::LOCALHOST, port).into()
}
fn nonce(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn config(path: &str) -> Result<(Vec<u16>, String), String> {
    let raw = fs::read(path).map_err(|_| "canary config unavailable")?;
    if raw.len() > 256 { return Err("canary config oversized".into()); }
    let text = std::str::from_utf8(&raw).map_err(|_| "canary config invalid")?;
    let fields: Vec<_> = text.split_whitespace().collect();
    if fields.len() != 3 || !nonce(fields[2]) { return Err("canary config invalid".into()); }
    let ports: Vec<u16> = fields[..2].iter().map(|s| s.parse().map_err(|_| "canary port invalid"))
        .collect::<Result<_, _>>()?;
    if ports.contains(&0) || ports[0] == ports[1] { return Err("canary ports invalid".into()); }
    Ok((ports, fields[2].to_string()))
}
fn connect(port: u16) -> std::io::Result<TcpStream> {
    TcpStream::connect_timeout(&addr(port), Duration::from_secs(1))
}
fn listen(root: &str, salt: &str) -> Result<(), String> {
    if !nonce(salt) { return Err("invalid canary nonce".into()); }
    let root = Path::new(root);
    let listeners = [TcpListener::bind(addr(0)), TcpListener::bind(addr(0))]
        .into_iter().collect::<Result<Vec<_>,_>>().map_err(|_| "canary bind failed")?;
    let ports = listeners.iter().map(|l| l.local_addr().map(|a| a.port()))
        .collect::<Result<Vec<_>,_>>().map_err(|_| "canary address unavailable")?;
    for l in &listeners { l.set_nonblocking(true).map_err(|_| "canary nonblocking failed")?; }
    let mut counts = [0u64; 3];
    fs::write(root.join("stats"), "0 0 0\n").map_err(|_| "canary stats failed")?;
    // Atomic readiness publishes both bound listeners and their synthetic nonce.
    fs::write(root.join("ready.tmp"), format!("{} {} {salt}\n", ports[0], ports[1])).map_err(|_| "canary readiness failed")?;
    fs::rename(root.join("ready.tmp"), root.join("ready")).map_err(|_| "canary readiness failed")?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(120) {
        for (i, listener) in listeners.iter().enumerate() {
            match listener.accept() {
                Ok((mut stream, peer)) => {
                    counts[i] += 1;
                    stream.set_read_timeout(Some(Duration::from_millis(200))).map_err(|_| "canary timeout failed")?;
                    stream.set_write_timeout(Some(Duration::from_millis(200))).map_err(|_| "canary timeout failed")?;
                    let mut raw = [0u8;64];
                    if !peer.ip().is_loopback() || stream.read_exact(&mut raw).is_err() || raw != salt.as_bytes() { counts[2] += 1; }
                    // Publish counter BEFORE ack: positive caller can then read exact baseline.
                    fs::write(root.join("stats.tmp"), format!("{} {} {}\n",counts[0],counts[1],counts[2])).map_err(|_| "canary stats failed")?;
                    fs::rename(root.join("stats.tmp"),root.join("stats")).map_err(|_| "canary stats failed")?;
                    let _ = stream.write_all(b"ok");
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => (),
                Err(_) => return Err("canary accept failed".into()),
            }
        }
        if root.join("stop").exists() { return Ok(()); }
        std::thread::sleep(Duration::from_millis(5));
    }
    Err("canary lifetime exceeded".into())
}
fn positive(path: &str) -> Result<(), String> {
    let (ports, salt) = config(path)?;
    for port in ports {
        let mut stream = connect(port).map_err(|_| "canary positive connect failed")?;
        stream.set_read_timeout(Some(Duration::from_secs(1))).map_err(|_| "canary timeout failed")?;
        stream.set_write_timeout(Some(Duration::from_secs(1))).map_err(|_| "canary timeout failed")?;
        stream.write_all(salt.as_bytes()).map_err(|_| "canary handshake failed")?;
        let mut ack = [0u8;2];
        stream.read_exact(&mut ack).map_err(|_| "canary ack failed")?;
        if &ack != b"ok" { return Err("canary ack invalid".into()); }
    }
    Ok(())
}
fn probe(path: &str) -> Result<(), String> {
    let (ports, _) = config(path)?;
    let local = match TcpListener::bind(addr(0)) {
        Ok(listener) => match connect(listener.local_addr().map_err(|_| "inner address unavailable")?.port()) {
            Ok(_) => "loopback-up",
            Err(_) => "loopback-unavailable",
        },
        Err(_) => "loopback-unavailable",
    };
    let mut results = Vec::new();
    for port in ports {
        results.push(match connect(port) {
            Ok(_) => "connected",
            Err(e) => match e.kind() {
                std::io::ErrorKind::ConnectionRefused => "refused",
                std::io::ErrorKind::TimedOut => "timeout",
                std::io::ErrorKind::NetworkUnreachable | std::io::ErrorKind::HostUnreachable => "unreachable",
                _ => "error",
            },
        });
    }
    println!("{},{},{}", results[0],results[1],local);
    if results.iter().any(|r| matches!(*r, "connected"|"error")) { return Err("canary isolation not established".into()); }
    Ok(())
}
pub fn run(args: &[String]) -> Result<(), String> {
    match args.get(1).map(String::as_str) {
        Some("canary-listen") if args.len()==4 => listen(&args[2],&args[3]),
        Some("canary-positive") if args.len()==3 => positive(&args[2]),
        Some("canary-probe") if args.len()==3 => probe(&args[2]),
        _ => Err("invalid canary command".into()),
    }
}
