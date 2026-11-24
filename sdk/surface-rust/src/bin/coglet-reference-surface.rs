//! Explicit test-only stdio bridge to the production semantic reference surface.
use coglet_surface_rust::{
    Surface,
    config::AdapterFixture,
    fixture_clock::{FixtureInput, Input},
    reference::SemanticBackend,
    transport::{self, ControlPeer},
};
use std::io::{Read, Write};

struct State(std::path::PathBuf);
impl Drop for State {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn emit(out: &mut impl Write, frames: Vec<Vec<u8>>) -> Result<(), String> {
    for frame in frames {
        transport::write_frame(out, &frame).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())
}
enum Driver {
    Direct(Box<Surface>),
    Paired(
        transport::InProcess<Surface>,
        transport::RuntimeQueue<Surface>,
    ),
}
impl Driver {
    fn drain(q: &mut transport::RuntimeQueue<Surface>) -> Result<Vec<Vec<u8>>, String> {
        let mut out = Vec::new();
        while let Some(f) = q.recv()? {
            out.push(f);
        }
        Ok(out)
    }
    fn receive(&mut self, f: &[u8], now: u64) -> Result<Vec<Vec<u8>>, String> {
        match self {
            Self::Direct(p) => p.receive(f, now),
            Self::Paired(p, q) => {
                q.send(f)?;
                p.poll(now)?;
                Self::drain(q)
            }
        }
    }
    fn tick(&mut self, now: u64) -> Result<Vec<Vec<u8>>, String> {
        match self {
            Self::Direct(p) => p.tick(now),
            Self::Paired(p, q) => {
                p.poll(now)?;
                Self::drain(q)
            }
        }
    }
    fn disconnect(&mut self, now: u64) -> Result<(), String> {
        match self {
            Self::Direct(p) => p.disconnect(now),
            Self::Paired(p, _) => p.disconnect(now),
        }
    }
}
fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paired = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["--stdio"] => false,
        ["--stdio", "--transport", "in-process"] => true,
        _ => return Err("fixture adapter requires --stdio [--transport in-process]".into()),
    };
    let path = std::env::var_os("COGLET_FIXTURE_CONFIG")
        .ok_or("explicit COGLET_FIXTURE_CONFIG required")?;
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(1_048_577)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 1_048_576 {
        return Err("fixture config too large".into());
    }
    let case = std::env::var("COGLET_CONFORMANCE_CASE").ok();
    let fixture = AdapterFixture::parse(
        std::str::from_utf8(&bytes).map_err(|e| e.to_string())?,
        case.as_deref(),
    )?;
    let state = State(std::env::temp_dir().join(format!(
        "coglet-reference-{}",
        coglet_surface_rust::ids::random_id()?
    )));
    let backend = SemanticBackend::new(fixture.speech, fixture.expression);
    let mut peer = Surface::open(fixture.config, &state.0, backend)?;
    let kill = peer.kill_switch();
    let (mut driver, initial) = if paired {
        let (p, mut q) = transport::in_process_pair(peer, 0)?;
        let out = Driver::drain(&mut q)?;
        (Driver::Paired(p, q), out)
    } else {
        let out = peer.start()?;
        (Driver::Direct(Box::new(peer)), out)
    };
    let kill_at = std::env::var("COGLET_FIXTURE_KILL_AT_MS")
        .ok()
        .map(|s| {
            s.parse::<u64>()
                .map_err(|e| e.to_string())
                .and_then(|v| v.checked_mul(1000).ok_or("kill time overflow".into()))
        })
        .transpose()?;
    let mut output = std::io::stdout().lock();
    // Durable initialization is complete; hello retains its wire deadline.
    eprintln!("COGLET_CONFORMANCE_READY");
    emit(&mut output, initial)?;
    let mut input = FixtureInput::open()?;
    let result = (|| {
        loop {
            let incoming = input.next_input()?;
            let now = input.now();
            if kill_at.is_some_and(|at| now >= at) && !kill.is_latched() {
                kill.trigger("fixture local kill");
            }
            let frames = match incoming {
                Some(Input::Frame(Ok(Some(f)))) => driver.receive(&f, now)?,
                Some(Input::Frame(Ok(None))) => break,
                Some(Input::Frame(Err(e)) | Input::ClockError(e)) => return Err(e),
                Some(Input::Advance(_)) | None => driver.tick(now)?,
            };
            emit(&mut output, frames)?;
            input.acknowledge()?;
        }
        Ok(())
    })();
    let now = input.now();
    let disconnected = driver.disconnect(now);
    result.and(disconnected)
}
fn main() {
    if let Err(e) = run() {
        eprintln!("coglet-reference-surface: {e}");
        std::process::exit(1);
    }
}
