//! Test-only shared-engine adapter. No fixtures, expected outputs or ambient guest I/O.
use coglet_engine::{
    Engine, Error, GuestMemory, Host, Instance, Limits, Result, StepBudget, StepInput,
};
use coglet_engine_wamr::Wamr;
use coglet_guest_rust::{Context, HostError, Operation};
use coglet_mock::engine::{Context as NativeContext, MockEngine, NativeGuest, NativeModule};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{self, BufRead, Write},
    time::{Duration, Instant},
};

const ECHO: &[u8] = b"coglet-reference-echo-v1";
const COUNTER: &[u8] = b"coglet-reference-counter-v1";
const WASI: &str = "wasi_snapshot_preview1";
const BADF: u32 = 8;
const NOENT: u32 = 44;
const DENIED: u32 = 76;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    guest: String,
    events_hex: String,
    step: u64,
    logical_time: u64,
    seed: u64,
    files: BTreeMap<String, String>,
    #[serde(default)]
    host_result: u32,
}
fn unhex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return Err(Error::InvalidArgument);
    }
    s.as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|p| {
            let s = std::str::from_utf8(p).map_err(|_| Error::InvalidArgument)?;
            u8::from_str_radix(s, 16).map_err(|_| Error::InvalidArgument)
        })
        .collect()
}
fn hex(b: &[u8]) -> String {
    b.iter().map(|b| format!("{b:02x}")).collect()
}
fn valid_path(p: &str) -> bool {
    p.starts_with('/')
        && !p.contains('\\')
        && !p.chars().any(|c| c.is_control())
        && p[1..].split('/').all(|c| !matches!(c, "" | "." | ".."))
        && ["/code/", "/data/", "/logs/"]
            .iter()
            .any(|m| p.starts_with(m))
}
fn writable(p: &str) -> bool {
    p.starts_with("/data/") && p != "/data/inbox.jsonl" && p != "/data/runtime/heard.cbor"
}
#[derive(Clone)]
struct Descriptor {
    path: String,
    position: usize,
    read: bool,
    write: bool,
    append: bool,
}
struct Overlay {
    files: BTreeMap<String, Vec<u8>>,
    calls: Vec<Value>,
    result: u32,
    descriptors: BTreeMap<u32, Descriptor>,
    next_fd: u32,
}
fn word(args: &[u64], n: usize) -> Result<u32> {
    u32::try_from(*args.get(n).ok_or(Error::InvalidArgument)?).map_err(|_| Error::InvalidPointer)
}
fn put32(m: &mut dyn GuestMemory, p: u32, n: u32) -> Result<()> {
    m.write(p, &n.to_le_bytes())
}
fn get32(m: &dyn GuestMemory, p: u32) -> Result<u32> {
    Ok(u32::from_le_bytes(
        m.read(p, 4)?
            .try_into()
            .map_err(|_| Error::InvalidPointer)?,
    ))
}
fn plus(p: u32, n: u32) -> Result<u32> {
    p.checked_add(n).ok_or(Error::InvalidPointer)
}
impl Host for Overlay {
    fn call(
        &mut self,
        module: &str,
        name: &str,
        a: &[u64],
        m: &mut dyn GuestMemory,
        budget: &mut StepBudget,
    ) -> Result<u32> {
        budget.check_deadline()?;
        if module == "coglet_v0" && matches!(name, "timer" | "emit" | "llm" | "log") && a.len() == 2
        {
            let bytes = m.read(word(a, 0)?, word(a, 1)?)?;
            if self.result == 0 {
                self.calls
                    .push(json!({"op":name,"argument_hex":hex(&bytes)}));
            }
            return Ok(self.result);
        }
        if module != WASI {
            return Err(Error::UnqualifiedModule);
        }
        let fd = word(a, 0)?;
        match name {
            "args_get" | "environ_get" => Ok(0),
            "args_sizes_get" | "environ_sizes_get" => {
                put32(m, fd, 0)?;
                put32(m, word(a, 1)?, 0)?;
                Ok(0)
            }
            "fd_prestat_get" => {
                if !(3..=5).contains(&fd) {
                    return Ok(BADF);
                }
                let mut stat = [0; 8];
                stat[4..].copy_from_slice(&5u32.to_le_bytes());
                m.write(word(a, 1)?, &stat)?;
                Ok(0)
            }
            "fd_prestat_dir_name" => {
                let path = match fd {
                    3 => b"/code",
                    4 => b"/data",
                    5 => b"/logs",
                    _ => return Ok(BADF),
                };
                if word(a, 2)? < path.len() as u32 {
                    return Ok(28);
                }
                m.write(word(a, 1)?, path)?;
                Ok(0)
            }
            "path_open" => {
                let root = match fd {
                    3 => "/code",
                    4 => "/data",
                    5 => "/logs",
                    _ => return Ok(BADF),
                };
                let raw = m.read(word(a, 2)?, word(a, 3)?)?;
                let Ok(relative) = std::str::from_utf8(&raw) else {
                    return Ok(28);
                };
                let path = format!("{root}/{relative}");
                if !valid_path(&path) {
                    return Ok(DENIED);
                }
                let flags = word(a, 4)?;
                let rights = *a.get(5).ok_or(Error::InvalidArgument)?;
                let read = rights & 2 != 0;
                let write = rights & 64 != 0;
                let append = word(a, 7)? & 1 != 0;
                let create = flags & 1 != 0;
                let truncate = flags & 8 != 0;
                if (write || create || truncate)
                    && !(writable(&path) || (path.starts_with("/logs/") && append && !truncate))
                {
                    return Ok(DENIED);
                }
                if flags & 2 != 0 {
                    return Ok(54);
                } // regular files only
                if create && flags & 4 != 0 && self.files.contains_key(&path) {
                    return Ok(20);
                }
                if !self.files.contains_key(&path) && !create {
                    return Ok(NOENT);
                }
                // Validate the output address before mutating the overlay.
                m.read(word(a, 8)?, 4)?;
                if truncate || !self.files.contains_key(&path) {
                    self.files.insert(path.clone(), Vec::new());
                }
                let new = self.next_fd;
                self.next_fd = new.checked_add(1).ok_or(Error::MemoryExhausted)?;
                self.descriptors.insert(
                    new,
                    Descriptor {
                        path,
                        position: 0,
                        read,
                        write,
                        append,
                    },
                );
                put32(m, word(a, 8)?, new)?;
                Ok(0)
            }
            "fd_close" => Ok(if self.descriptors.remove(&fd).is_some() {
                0
            } else {
                BADF
            }),
            "fd_filestat_get" => {
                let Some(d) = self.descriptors.get(&fd) else {
                    return Ok(BADF);
                };
                let mut stat = [0; 64];
                stat[16] = 4;
                stat[32..40].copy_from_slice(&(self.files[&d.path].len() as u64).to_le_bytes());
                m.write(word(a, 1)?, &stat)?;
                Ok(0)
            }
            "fd_fdstat_get" => {
                let mut stat = [0; 24];
                if (3..=5).contains(&fd) {
                    stat[0] = 3;
                    stat[8..16].copy_from_slice(&(1u64 << 13).to_le_bytes()); // path_open
                    let read_rights = 2u64 | 4 | 32 | 128 | (1 << 21);
                    let rights = read_rights | if fd == 3 { 0 } else { 1 | 16 | 64 };
                    stat[16..24].copy_from_slice(&rights.to_le_bytes());
                } else if let Some(d) = self.descriptors.get(&fd) {
                    stat[0] = 4;
                    stat[2] = u8::from(d.append);
                    stat[8..16].copy_from_slice(
                        &((if d.read { 2u64 } else { 0 }) | (if d.write { 64 } else { 0 }))
                            .to_le_bytes(),
                    );
                } else {
                    return Ok(BADF);
                }
                m.write(word(a, 1)?, &stat)?;
                Ok(0)
            }
            "fd_read" | "fd_write" => {
                let writing = name == "fd_write";
                if fd == 0 && !writing {
                    put32(m, word(a, 3)?, 0)?;
                    return Ok(0);
                }
                let mut d = if writing && (fd == 1 || fd == 2) {
                    Descriptor {
                        path: format!("/logs/{}", if fd == 1 { "stdout" } else { "stderr" }),
                        position: 0,
                        read: false,
                        write: true,
                        append: true,
                    }
                } else if let Some(d) = self.descriptors.get(&fd) {
                    d.clone()
                } else {
                    return Ok(BADF);
                };
                if (writing && !d.write) || (!writing && !d.read) {
                    return Ok(DENIED);
                }
                let vectors = word(a, 1)?;
                let n = word(a, 2)?;
                if n > 1024 {
                    return Ok(28);
                }
                let mut total = 0u32;
                for i in 0..n {
                    let v = plus(vectors, i * 8)?;
                    let p = get32(m, v)?;
                    let len = get32(m, plus(v, 4)?)?;
                    if writing {
                        let bytes = m.read(p, len)?;
                        let file = self.files.entry(d.path.clone()).or_default();
                        if d.append {
                            d.position = file.len();
                        }
                        let end = d
                            .position
                            .checked_add(bytes.len())
                            .ok_or(Error::MemoryExhausted)?;
                        if end > 4 * 1024 * 1024 {
                            return Ok(51);
                        }
                        file.resize(file.len().max(end), 0);
                        file[d.position..end].copy_from_slice(&bytes);
                        d.position = end;
                        total = total.checked_add(len).ok_or(Error::InvalidArgument)?;
                    } else {
                        let file = &self.files[&d.path];
                        let start = d.position.min(file.len());
                        let count = (len as usize).min(file.len() - start);
                        m.write(p, &file[start..start + count])?;
                        d.position += count;
                        total = total
                            .checked_add(count as u32)
                            .ok_or(Error::InvalidArgument)?;
                        if count < len as usize {
                            break;
                        }
                    }
                }
                if fd > 5 {
                    self.descriptors.insert(fd, d);
                }
                put32(m, word(a, 3)?, total)?;
                Ok(0)
            }
            "fd_seek" => {
                let Some(d) = self.descriptors.get_mut(&fd) else {
                    return Ok(BADF);
                };
                let offset = *a.get(1).ok_or(Error::InvalidArgument)? as i64;
                let base = match word(a, 2)? {
                    0 => 0,
                    1 => d.position as i128,
                    2 => self.files[&d.path].len() as i128,
                    _ => return Ok(28),
                };
                let pos = base + offset as i128;
                if !(0..=4 * 1024 * 1024).contains(&pos) {
                    return Ok(28);
                }
                d.position = pos as usize;
                m.write(word(a, 3)?, &(d.position as u64).to_le_bytes())?;
                Ok(0)
            }
            "fd_sync" | "fd_datasync" | "fd_advise" => Ok(if self.descriptors.contains_key(&fd) {
                0
            } else {
                BADF
            }),
            "proc_exit" => Err(Error::GuestExit(fd)),
            _ => Err(Error::UnqualifiedModule),
        }
    }
}

// SDK effects and file copies pass through NativeContext's metered Host boundary,
// not directly into the overlay. Cooperative compute is simulation, not Wasm fuel.
struct NativeHost<'a, 'b> {
    cx: &'a mut NativeContext<'b>,
}
impl NativeHost<'_, '_> {
    fn wasi(&mut self, name: &str, args: &[u64]) -> std::result::Result<u32, HostError> {
        self.cx
            .charge(1)
            .map_err(|_| HostError::ResourceExhausted)?;
        self.cx
            .call(WASI, name, args)
            .map_err(|_| HostError::ResourceExhausted)
    }
    fn open(&mut self, path: &str, write: bool) -> std::result::Result<Option<u32>, HostError> {
        if !valid_path(path) {
            return Err(HostError::PermissionDenied);
        }
        let (root, relative) = path[1..]
            .split_once('/')
            .ok_or(HostError::InvalidArgument)?;
        let fd = match root {
            "code" => 3,
            "data" => 4,
            "logs" => 5,
            _ => return Err(HostError::PermissionDenied),
        };
        self.cx
            .memory
            .write(4096, relative.as_bytes())
            .map_err(|_| HostError::ResourceExhausted)?;
        let code = self.wasi(
            "path_open",
            &[
                fd,
                0,
                4096,
                relative.len() as u64,
                if write { 9 } else { 0 },
                if write { 64 } else { 2 },
                0,
                0,
                5120,
            ],
        )?;
        match code {
            0 => Ok(Some(
                get32(&self.cx.memory, 5120).map_err(|_| HostError::ResourceExhausted)?,
            )),
            NOENT => Ok(None),
            _ => Err(HostError::PermissionDenied),
        }
    }
}
impl coglet_guest_rust::Host for NativeHost<'_, '_> {
    fn call(&mut self, operation: Operation, bytes: &[u8]) -> i32 {
        let result = (|| -> Result<u32> {
            self.cx.charge(1 + bytes.len() as u64)?;
            self.cx.memory.write(8192, bytes)?;
            self.cx.call(
                "coglet_v0",
                match operation {
                    Operation::Timer => "timer",
                    Operation::Emit => "emit",
                    Operation::Llm => "llm",
                    Operation::Log => "log",
                },
                &[8192, bytes.len() as u64],
            )
        })();
        result.unwrap_or(3) as i32
    }
    fn read_file(&mut self, path: &str) -> std::result::Result<Option<Vec<u8>>, HostError> {
        let Some(fd) = self.open(path, false)? else {
            return Ok(None);
        };
        let result = (|| {
            let mut bytes = Vec::new();
            loop {
                self.cx
                    .memory
                    .write(
                        5136,
                        &[8192u32.to_le_bytes(), 4096u32.to_le_bytes()].concat(),
                    )
                    .map_err(|_| HostError::ResourceExhausted)?;
                if self.wasi("fd_read", &[fd as u64, 5136, 1, 5120])? != 0 {
                    return Err(HostError::PermissionDenied);
                }
                let n = get32(&self.cx.memory, 5120).map_err(|_| HostError::ResourceExhausted)?;
                bytes.extend(
                    self.cx
                        .memory
                        .read(8192, n)
                        .map_err(|_| HostError::ResourceExhausted)?,
                );
                self.cx
                    .charge(n as u64 + 1)
                    .map_err(|_| HostError::ResourceExhausted)?;
                if n < 4096 {
                    break;
                }
            }
            Ok(Some(bytes))
        })();
        let close = self.wasi("fd_close", &[fd as u64]);
        close?;
        result
    }
    fn write_file(&mut self, path: &str, bytes: &[u8]) -> std::result::Result<(), HostError> {
        let fd = self.open(path, true)?.ok_or(HostError::PermissionDenied)?;
        let result = (|| {
            self.cx
                .charge(bytes.len() as u64 + 1)
                .map_err(|_| HostError::ResourceExhausted)?;
            self.cx
                .memory
                .write(8192, bytes)
                .map_err(|_| HostError::ResourceExhausted)?;
            let len = u32::try_from(bytes.len()).map_err(|_| HostError::ResourceExhausted)?;
            self.cx
                .memory
                .write(5136, &[8192u32.to_le_bytes(), len.to_le_bytes()].concat())
                .map_err(|_| HostError::ResourceExhausted)?;
            if self.wasi("fd_write", &[fd as u64, 5136, 1, 5120])? != 0 {
                return Err(HostError::PermissionDenied);
            }
            Ok(())
        })();
        let close = self.wasi("fd_close", &[fd as u64]);
        close?;
        result
    }
}
struct ReferenceGuest {
    echo: bool,
}
impl NativeGuest for ReferenceGuest {
    fn handle(&mut self, ptr: u32, input: &StepInput, cx: &mut NativeContext<'_>) -> Result<u32> {
        cx.charge(input.events.len() as u64 + 1)?;
        let bytes = cx.memory.read(
            ptr,
            u32::try_from(input.events.len()).map_err(|_| Error::InvalidPointer)?,
        )?;
        let Ok(events) = coglet_guest_rust::decode_events(&bytes) else {
            return Ok(1);
        };
        cx.charge(events.len() as u64 + 1)?;
        let mut host = NativeHost { cx };
        let mut context = Context::new(&mut host, input.step, input.logical_time, input.seed);
        let result = if self.echo {
            coglet_echo::wake(&mut context, &events)
        } else {
            coglet_counter::wake(&mut context, &events)
        };
        Ok(u32::from(result.is_err()))
    }
}
fn execute<E: Engine>(e: &E, module: &[u8], input: StepInput, host: &mut Overlay) -> Result<()> {
    let limits = Limits {
        compute_mode: e.metadata().compute_mode.into(),
        compute: 10_000_000,
        memory_pages: 256,
        host_bytes: 1024 * 1024,
        runtime_bytes: 32 * 1024 * 1024,
        stack_bytes: 256 * 1024,
        deadline: Instant::now() + Duration::from_secs(1),
    };
    let m = e.load(module, &limits)?;
    e.instantiate(&m, limits)?.handle(input, host).result
}
fn run(mode: &str, r: Request) -> Result<Value> {
    let mut files = BTreeMap::new();
    for (path, data) in &r.files {
        if !valid_path(path) {
            return Err(Error::InvalidArgument);
        }
        files.insert(path.clone(), unhex(data)?);
    }
    let original = files.clone();
    let mut host = Overlay {
        files,
        calls: Vec::new(),
        result: r.host_result,
        descriptors: BTreeMap::new(),
        next_fd: 6,
    };
    let input = StepInput {
        events: unhex(&r.events_hex)?,
        step: r.step,
        logical_time: r.logical_time,
        seed: r.seed,
    };
    let key = match r.guest.as_str() {
        "echo" => ECHO,
        "counter" => COUNTER,
        _ => return Err(Error::InvalidArgument),
    };
    let result = if mode == "mock" {
        let mut e = MockEngine::default();
        e.register(
            ECHO.to_vec(),
            NativeModule::new(32, 256, || Box::new(ReferenceGuest { echo: true }))?,
        )?;
        e.register(
            COUNTER.to_vec(),
            NativeModule::new(32, 256, || Box::new(ReferenceGuest { echo: false }))?,
        )?;
        execute(&e, key, input, &mut host)
    } else {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(format!(
            "../../target/guests/wasm32-wasip1/release/coglet_{}.wasm",
            r.guest
        ));
        let module = std::fs::read(&path).map_err(|e| {
            Error::Trap(format!(
                "{}: {e}; run sdk/guest-rust/build-guests.sh",
                path.display()
            ))
        })?;
        execute(&Wamr::interpreter(), &module, input, &mut host)
    };
    if let Err(e) = &result {
        eprintln!("{mode}/{}: {e}", r.guest);
    }
    let ok = result.is_ok();
    let files: BTreeMap<_, _> = (if ok { &host.files } else { &original })
        .iter()
        .map(|(k, v)| (k.clone(), hex(v)))
        .collect();
    Ok(json!({"ok":ok, "calls":if ok {host.calls} else {Vec::new()}, "files":files}))
}
fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args()
        .nth(1)
        .ok_or("usage: reference_conformance mock|wamr")?;
    if mode != "mock" && mode != "wamr" {
        return Err("expected mock|wamr".into());
    }
    let mut stdout = io::stdout().lock();
    for line in io::stdin().lock().lines() {
        let line = line?;
        if line.len() > 8 * 1024 * 1024 {
            return Err("oversize request".into());
        }
        let response = match serde_json::from_str::<Request>(&line) {
            Ok(r) => {
                let original = r.files.clone();
                match run(&mode, r) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("adapter: {e}");
                        json!({"ok":false,"calls":[],"files":original})
                    }
                }
            }
            Err(e) => {
                eprintln!("adapter: {e}");
                json!({"ok":false,"calls":[],"files":{}})
            }
        };
        serde_json::to_writer(&mut stdout, &response)?;
        writeln!(stdout)?;
        stdout.flush()?;
    }
    Ok(())
}
