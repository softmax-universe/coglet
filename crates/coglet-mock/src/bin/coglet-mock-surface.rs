//! Fixture-only framed stdio adapter; stdout contains protocol frames only.
use coglet_mock::surface::{MAX_CONFIG_BYTES, MockSurface};
use coglet_surface_rust::fixture_clock::{FixtureInput, Input};
use coglet_surface_rust::transport::{self, ControlPeer};
use std::io::{Read, Write};
use std::path::PathBuf;

struct StateDir(PathBuf);
impl StateDir {
    fn create() -> Result<Self, String> {
        let path = std::env::temp_dir().join(format!(
            "coglet-mock-surface-{}",
            coglet_surface_rust::ids::random_id()?
        ));
        let mut builder = std::fs::DirBuilder::new();
        #[cfg(unix)]
        {
            use std::os::unix::fs::DirBuilderExt;
            builder.mode(0o700);
        }
        builder.create(&path).map_err(|e| e.to_string())?;
        Ok(Self(path))
    }
}
impl Drop for StateDir {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_dir_all(&self.0) {
            eprintln!("state cleanup: {e}");
        }
    }
}

enum Driver {
    Direct(Box<MockSurface>),
    Paired(
        transport::InProcess<MockSurface>,
        transport::RuntimeQueue<MockSurface>,
    ),
}
impl Driver {
    fn drain(queue: &mut transport::RuntimeQueue<MockSurface>) -> Result<Vec<Vec<u8>>, String> {
        let mut out = Vec::new();
        while let Some(frame) = queue.recv()? {
            out.push(frame);
        }
        Ok(out)
    }
    fn receive(&mut self, frame: &[u8], now: u64) -> Result<Vec<Vec<u8>>, String> {
        match self {
            Self::Direct(peer) => peer.receive(frame, now),
            Self::Paired(peer, queue) => {
                queue.send(frame)?;
                peer.poll(now)?;
                Self::drain(queue)
            }
        }
    }
    fn tick(&mut self, now: u64) -> Result<Vec<Vec<u8>>, String> {
        match self {
            Self::Direct(peer) => peer.tick(now),
            Self::Paired(peer, queue) => {
                peer.poll(now)?;
                Self::drain(queue)
            }
        }
    }
    fn disconnect(&mut self, now: u64) -> Result<(), String> {
        match self {
            Self::Direct(peer) => peer.disconnect(now),
            Self::Paired(peer, _) => peer.disconnect(now),
        }
    }
}
fn emit(out: &mut impl Write, frames: Vec<Vec<u8>>) -> Result<(), String> {
    for frame in frames {
        transport::write_frame(out, &frame).map_err(|e| e.to_string())?;
    }
    out.flush().map_err(|e| e.to_string())
}
fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let mut stdio = false;
    let mut in_process = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--stdio" => stdio = true,
            "--transport" => match args.next().as_deref() {
                Some("in-process") => in_process = true,
                Some("stdio") => in_process = false,
                _ => return Err("--transport expects stdio or in-process".into()),
            },
            _ => return Err(format!("unknown argument: {arg}")),
        }
    }
    if !stdio {
        return Err("fixture adapter requires --stdio".into());
    }
    let config_path = std::env::var_os("COGLET_FIXTURE_CONFIG")
        .ok_or("COGLET_FIXTURE_CONFIG must explicitly name a fixture file")?;
    let mut bytes = Vec::new();
    std::fs::File::open(config_path)
        .map_err(|e| e.to_string())?
        .take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err("fixture config too large".into());
    }
    let src = std::str::from_utf8(&bytes).map_err(|e| e.to_string())?;
    let kill_at = std::env::var("COGLET_FIXTURE_KILL_AT_MS")
        .ok()
        .map(|s| {
            s.parse::<u64>()
                .map_err(|e| e.to_string())
                .and_then(|ms| ms.checked_mul(1000).ok_or("kill time overflow".into()))
        })
        .transpose()?;
    let state = StateDir::create()?;
    let mut peer = MockSurface::from_fixture(
        src,
        std::env::var("COGLET_CONFORMANCE_CASE")
            .or_else(|_| std::env::var("COGLET_FIXTURE_CASE"))
            .ok()
            .as_deref(),
        &state.0,
    )?;
    let kill = peer.core().kill_switch();
    let mut output = std::io::stdout().lock();
    let (mut driver, initial) = if in_process {
        let (p, mut q) = transport::in_process_pair(peer, 0)?;
        let initial = Driver::drain(&mut q)?;
        (Driver::Paired(p, q), initial)
    } else {
        let initial = peer.start()?;
        (Driver::Direct(Box::new(peer)), initial)
    };
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
                Some(Input::Frame(Ok(Some(frame)))) => driver.receive(&frame, now)?,
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
        eprintln!("coglet-mock-surface: {e}");
        std::process::exit(1);
    }
}
