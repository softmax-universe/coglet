//! Restricted WASI preview1 file ops on a coglet-fs copy-on-write overlay.
//! No ambient host paths, sockets, clocks, or random.
use crate::Snapshot;
use coglet_engine::{Error, GuestMemory, Result, StepBudget};
use coglet_fs::Transaction;
use std::collections::BTreeMap;

pub(super) const NOTCAPABLE: u32 = 76;
const BADF: u32 = 8;
const EXIST: u32 = 20;
const INVAL: u32 = 28;
const NOENT: u32 = 44;
const NOSPC: u32 = 51;
const NOTDIR: u32 = 54;
const FD_READ: u64 = 2;
const FD_WRITE: u64 = 64;
const OFLAG_CREAT: u32 = 1;
const OFLAG_DIRECTORY: u32 = 2;
const OFLAG_EXCL: u32 = 4;
const OFLAG_TRUNC: u32 = 8;
const FDFLAG_APPEND: u32 = 1;

#[derive(Clone)]
enum Fd {
    Stdin,
    Stdout,
    Stderr,
    Preopen(&'static str),
    File {
        path: String,
        pos: usize,
        read: bool,
        write: bool,
        append: bool,
    },
}

pub(super) struct Wasi {
    overlay: Transaction,
    fds: BTreeMap<u32, Fd>,
    next_fd: u32,
}

impl Wasi {
    pub(super) fn new(files: &Snapshot) -> crate::Result<Self> {
        let overlay = files.decoded_fs()?.transaction();
        Ok(Self {
            overlay,
            fds: BTreeMap::from([
                (0, Fd::Stdin),
                (1, Fd::Stdout),
                (2, Fd::Stderr),
                (3, Fd::Preopen("/code")),
                (4, Fd::Preopen("/data")),
                (5, Fd::Preopen("/logs")),
            ]),
            next_fd: 6,
        })
    }

    pub(super) fn commit(self, files: &mut Snapshot) -> crate::Result<()> {
        *files = files.from_fs_after(self.overlay.commit())?;
        Ok(())
    }

    pub(super) fn sync_events_log(&mut self, files: &Snapshot) -> crate::Result<()> {
        let Some(encoded) = files.files().get("/logs/events.jsonl") else {
            return Ok(());
        };
        let bytes = crate::filesystem::decode_hex(encoded)?;
        let current = self
            .overlay
            .snapshot()
            .read(coglet_fs::EVENTS)
            .unwrap_or(&[]);
        if bytes.len() < current.len() || !bytes.starts_with(current) {
            return Err("events log diverged from overlay".into());
        }
        if bytes.len() > current.len() {
            self.overlay
                .host_append_log(&bytes[current.len()..])
                .map_err(fs_string)?;
        }
        Ok(())
    }

    pub(super) fn call(
        &mut self,
        name: &str,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
    ) -> Result<u32> {
        budget.check_deadline()?;
        match name {
            "args_get" | "environ_get" => Ok(0),
            "args_sizes_get" | "environ_sizes_get" => {
                put32(memory, u32arg(args, 0)?, 0)?;
                put32(memory, u32arg(args, 1)?, 0)?;
                Ok(0)
            }
            "fd_prestat_get" => self.fd_prestat_get(args, memory),
            "fd_prestat_dir_name" => self.fd_prestat_dir_name(args, memory),
            "path_open" => self.path_open(args, memory),
            "fd_close" => self.fd_close(args),
            "fd_read" => self.fd_read_write(args, memory, budget, false),
            "fd_write" => self.fd_read_write(args, memory, budget, true),
            "fd_pread" => self.fd_pread_pwrite(args, memory, budget, false),
            "fd_pwrite" => self.fd_pread_pwrite(args, memory, budget, true),
            "fd_seek" => self.fd_seek(args, memory),
            "fd_tell" => self.fd_tell(args, memory),
            "fd_fdstat_get" => self.fd_fdstat_get(args, memory),
            "fd_filestat_get" => self.fd_filestat_get(args, memory),
            "path_filestat_get" => self.path_filestat_get(args, memory),
            "fd_readdir" => self.fd_readdir(args, memory),
            "fd_sync" | "fd_datasync" | "fd_advise" => self.require_fd(u32arg(args, 0)?),
            "fd_allocate"
            | "fd_fdstat_set_flags"
            | "fd_fdstat_set_rights"
            | "fd_filestat_set_times"
            | "path_filestat_set_times"
            | "path_create_directory"
            | "path_remove_directory" => Ok(NOTCAPABLE),
            "fd_filestat_set_size" => self.fd_set_size(args, budget),
            "path_unlink_file" => self.path_unlink(args, memory),
            "path_rename" => self.path_rename(args, memory),
            "path_readlink" => {
                let _ =
                    self.resolve(u32arg(args, 0)?, memory, u32arg(args, 1)?, u32arg(args, 2)?)?;
                Ok(NOTCAPABLE)
            }
            "fd_renumber" => self.fd_renumber(args),
            "proc_exit" => Err(Error::GuestExit(u32arg(args, 0)?)),
            _ => Err(Error::UnqualifiedModule),
        }
    }

    fn fd_prestat_get(&self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let Some(Fd::Preopen(root)) = self.fds.get(&u32arg(args, 0)?) else {
            return Ok(BADF);
        };
        let mut stat = [0u8; 8];
        stat[4..8].copy_from_slice(&(root.len() as u32).to_le_bytes());
        memory.write(u32arg(args, 1)?, &stat)?;
        Ok(0)
    }

    fn fd_prestat_dir_name(&self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let Some(Fd::Preopen(root)) = self.fds.get(&u32arg(args, 0)?) else {
            return Ok(BADF);
        };
        let bytes = root.as_bytes();
        if u32arg(args, 2)? < bytes.len() as u32 {
            return Ok(INVAL);
        }
        memory.write(u32arg(args, 1)?, bytes)?;
        Ok(0)
    }

    fn path_open(&mut self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let path =
            match self.resolve(u32arg(args, 0)?, memory, u32arg(args, 2)?, u32arg(args, 3)?)? {
                Ok(path) => path,
                Err(errno) => return Ok(errno),
            };
        let oflags = u32arg(args, 4)?;
        let rights = *args.get(5).ok_or(Error::InvalidArgument)?;
        let fdflags = u32arg(args, 7)?;
        let read = rights & FD_READ != 0;
        let write = rights & FD_WRITE != 0;
        let append = fdflags & FDFLAG_APPEND != 0;
        let create = oflags & OFLAG_CREAT != 0;
        let exclusive = oflags & OFLAG_EXCL != 0;
        let truncate = oflags & OFLAG_TRUNC != 0;
        if oflags & OFLAG_DIRECTORY != 0 {
            return Ok(NOTDIR);
        }
        if !open_allowed(&path, write || create || truncate, append && !truncate) {
            return Ok(NOTCAPABLE);
        }
        let exists = self.overlay.snapshot().read(&path).is_ok();
        if exists && create && exclusive {
            return Ok(EXIST);
        }
        if !exists && !create {
            return Ok(NOENT);
        }
        memory.read(u32arg(args, 8)?, 4)?;
        if !exists {
            let created = fs_errno(if path.starts_with("/logs/") {
                self.overlay.create(&path)
            } else {
                self.overlay.write(&path, &[])
            });
            if let Err(errno) = created {
                return Ok(errno);
            }
        } else if truncate {
            let truncated = fs_errno(self.overlay.truncate(&path, 0));
            if let Err(errno) = truncated {
                return Ok(errno);
            }
        }
        while self.fds.contains_key(&self.next_fd) {
            self.next_fd = self.next_fd.checked_add(1).ok_or(Error::MemoryExhausted)?;
        }
        let fd = self.next_fd;
        self.next_fd = fd.checked_add(1).ok_or(Error::MemoryExhausted)?;
        self.fds.insert(
            fd,
            Fd::File {
                path,
                pos: 0,
                read,
                write,
                append,
            },
        );
        put32(memory, u32arg(args, 8)?, fd)?;
        Ok(0)
    }

    fn fd_close(&mut self, args: &[u64]) -> Result<u32> {
        let fd = u32arg(args, 0)?;
        Ok(if self.fds.remove(&fd).is_some() {
            0
        } else {
            BADF
        })
    }

    fn fd_read_write(
        &mut self,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
        writing: bool,
    ) -> Result<u32> {
        let fd = u32arg(args, 0)?;
        let mut desc = match self.stdio_or_file(fd, writing)? {
            Ok(desc) => desc,
            Err(errno) => return Ok(errno),
        };
        let (total, pos) = match self.transfer(
            &mut desc,
            memory,
            budget,
            (u32arg(args, 1)?, u32arg(args, 2)?),
            writing,
            None,
        )? {
            Ok(v) => v,
            Err(errno) => return Ok(errno),
        };
        if let Fd::File { pos: stored, .. } = &mut desc {
            *stored = pos;
        }
        if fd > 5 {
            self.fds.insert(fd, desc);
        }
        put32(memory, u32arg(args, 3)?, total)?;
        Ok(0)
    }

    fn fd_pread_pwrite(
        &mut self,
        args: &[u64],
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
        writing: bool,
    ) -> Result<u32> {
        let fd = u32arg(args, 0)?;
        let mut desc = match self.stdio_or_file(fd, writing)? {
            Ok(desc) => desc,
            Err(errno) => return Ok(errno),
        };
        let offset = *args.get(3).ok_or(Error::InvalidArgument)?;
        if offset > usize::MAX as u64 {
            return Ok(INVAL);
        }
        let (total, _) = match self.transfer(
            &mut desc,
            memory,
            budget,
            (u32arg(args, 1)?, u32arg(args, 2)?),
            writing,
            Some(offset as usize),
        )? {
            Ok(v) => v,
            Err(errno) => return Ok(errno),
        };
        put32(memory, u32arg(args, 4)?, total)?;
        Ok(0)
    }

    fn transfer(
        &mut self,
        desc: &mut Fd,
        memory: &mut dyn GuestMemory,
        budget: &mut StepBudget,
        (iovs, n): (u32, u32),
        writing: bool,
        at: Option<usize>,
    ) -> Result<std::result::Result<(u32, usize), u32>> {
        let (path, mut pos, readable, writable, append) = match desc {
            Fd::Stdin if !writing => return Ok(Ok((0, 0))),
            Fd::Stdout if writing => ("/logs/stdout".to_owned(), 0, false, true, true),
            Fd::Stderr if writing => ("/logs/stderr".to_owned(), 0, false, true, true),
            Fd::File {
                path,
                pos,
                read,
                write,
                append,
            } => (path.clone(), *pos, *read, *write, *append),
            _ => return Ok(Err(BADF)),
        };
        if (writing && !writable) || (!writing && !readable) {
            return Ok(Err(NOTCAPABLE));
        }
        if n > 1024 {
            return Ok(Err(INVAL));
        }
        let mut total = 0u32;
        let mut cursor = at.unwrap_or(pos);
        // The engine bridge charges guest bytes copied in this call only after
        // the host call returns; count them here so a hole reservation cannot
        // exceed the remaining allowance before allocation.
        let mut copied = 0u64;
        for i in 0..n {
            let vec = plus(iovs, i.checked_mul(8).ok_or(Error::InvalidPointer)?)?;
            let ptr = get32(memory, vec)?;
            let len = get32(memory, plus(vec, 4)?)?;
            if writing {
                let current = self.file_len(&path);
                let positional = !(append && at.is_none());
                // A positional write past EOF zero-fills a host-side hole that is
                // never copied from guest memory: reserve it before allocating.
                let hole = if positional {
                    cursor.saturating_sub(current) as u64
                } else {
                    0
                };
                let pending = copied
                    .checked_add(u64::from(len))
                    .and_then(|v| v.checked_add(hole))
                    .ok_or(Error::HostBytesExhausted)?;
                if pending > budget.host_bytes_remaining() {
                    return Err(Error::HostBytesExhausted);
                }
                budget.charge_host_bytes(hole)?;
                let bytes = memory.read(ptr, len)?;
                copied += u64::from(len);
                let result = if positional {
                    self.overlay.write_at(&path, cursor, &bytes)
                } else {
                    cursor = current;
                    self.overlay.append(&path, &bytes)
                };
                if let Err(errno) = fs_errno(result) {
                    return Ok(Err(errno));
                }
                cursor = cursor.saturating_add(bytes.len());
                total = total.checked_add(len).ok_or(Error::InvalidArgument)?;
            } else {
                let file = match self.overlay.snapshot().read(&path) {
                    Ok(bytes) => bytes,
                    Err(e) => return Ok(Err(fs_code(e))),
                };
                let start = cursor.min(file.len());
                let count = (len as usize).min(file.len() - start);
                memory.write(ptr, &file[start..start + count])?;
                cursor += count;
                total = total
                    .checked_add(count as u32)
                    .ok_or(Error::InvalidArgument)?;
                if count < len as usize {
                    break;
                }
            }
        }
        if at.is_none() {
            pos = cursor;
        }
        Ok(Ok((total, pos)))
    }

    fn fd_seek(&mut self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let fd = u32arg(args, 0)?;
        let Some(Fd::File { path, pos, .. }) = self.fds.get_mut(&fd) else {
            return Ok(BADF);
        };
        let offset = *args.get(1).ok_or(Error::InvalidArgument)? as i64 as i128;
        let base = match u32arg(args, 2)? {
            0 => 0,
            1 => *pos as i128,
            2 => match self.overlay.snapshot().read(path) {
                Ok(bytes) => bytes.len() as i128,
                Err(e) => return Ok(fs_code(e)),
            },
            _ => return Ok(INVAL),
        };
        let next = base + offset;
        if !(0..=i128::from(u32::MAX)).contains(&next) {
            return Ok(INVAL);
        }
        *pos = next as usize;
        put64(memory, u32arg(args, 3)?, *pos as u64)?;
        Ok(0)
    }

    fn fd_tell(&self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let Some(Fd::File { pos, .. }) = self.fds.get(&u32arg(args, 0)?) else {
            return Ok(BADF);
        };
        put64(memory, u32arg(args, 1)?, *pos as u64)?;
        Ok(0)
    }

    fn fd_fdstat_get(&self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let fd = u32arg(args, 0)?;
        let mut stat = [0u8; 24];
        match self.fds.get(&fd) {
            Some(Fd::Stdin) => {
                stat[0] = 2;
                stat[8..16].copy_from_slice(&FD_READ.to_le_bytes());
            }
            Some(Fd::Stdout | Fd::Stderr) => {
                stat[0] = 2;
                stat[2] = FDFLAG_APPEND as u8;
                stat[8..16].copy_from_slice(&FD_WRITE.to_le_bytes());
            }
            Some(Fd::Preopen(root)) => {
                stat[0] = 3;
                let mut base = (1u64 << 13) | (1 << 14) | (1 << 18) | (1 << 21);
                if *root != "/code" {
                    base |= 1 << 10; // PATH_CREATE_FILE
                }
                if *root == "/data" {
                    base |= (1 << 16) | (1 << 17) | (1 << 26);
                }
                stat[8..16].copy_from_slice(&base.to_le_bytes());
                let read_rights = FD_READ | 4 | 32 | 128 | (1 << 21);
                let rights = read_rights
                    | if *root == "/code" {
                        0
                    } else {
                        1 | 16 | FD_WRITE
                    };
                stat[16..24].copy_from_slice(&rights.to_le_bytes());
            }
            Some(Fd::File {
                read,
                write,
                append,
                ..
            }) => {
                stat[0] = 4;
                stat[2] = u8::from(*append);
                stat[8..16].copy_from_slice(
                    &((if *read { FD_READ } else { 0 }) | (if *write { FD_WRITE } else { 0 }))
                        .to_le_bytes(),
                );
            }
            None => return Ok(BADF),
        }
        memory.write(u32arg(args, 1)?, &stat)?;
        Ok(0)
    }

    fn fd_filestat_get(&self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let fd = u32arg(args, 0)?;
        let (filetype, size) = match self.fds.get(&fd) {
            Some(Fd::Stdin | Fd::Stdout | Fd::Stderr) => (2u8, 0u64),
            Some(Fd::Preopen(_)) => (3, 0),
            Some(Fd::File { path, .. }) => match self.overlay.snapshot().read(path) {
                Ok(bytes) => (4u8, bytes.len() as u64),
                Err(e) => return Ok(fs_code(e)),
            },
            None => return Ok(BADF),
        };
        memory.write(u32arg(args, 1)?, &filestat(filetype, size))?;
        Ok(0)
    }

    fn path_filestat_get(&self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let path =
            match self.resolve(u32arg(args, 0)?, memory, u32arg(args, 2)?, u32arg(args, 3)?)? {
                Ok(path) => path,
                Err(errno) => return Ok(errno),
            };
        match self.overlay.snapshot().read(&path) {
            Ok(bytes) => {
                memory.write(u32arg(args, 4)?, &filestat(4, bytes.len() as u64))?;
                Ok(0)
            }
            Err(e) => Ok(fs_code(e)),
        }
    }

    fn fd_readdir(&self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let fd = u32arg(args, 0)?;
        let prefix = match self.fds.get(&fd) {
            Some(Fd::Preopen(root)) => (*root).to_owned(),
            Some(Fd::File { .. }) => return Ok(NOTDIR),
            _ => return Ok(BADF),
        };
        let mut entries = BTreeMap::new();
        let pfx = format!("{prefix}/");
        for (path, _) in self.overlay.snapshot().files() {
            if let Some(rest) = path.strip_prefix(&pfx) {
                let name = rest.split('/').next().unwrap_or(rest);
                entries.insert(name.to_owned(), rest.contains('/'));
            }
        }
        let cookie = *args.get(3).ok_or(Error::InvalidArgument)?;
        let buf = u32arg(args, 1)?;
        let len = u32arg(args, 2)? as usize;
        let mut used = 0usize;
        for (index, (name, is_dir)) in entries.into_iter().enumerate() {
            if (index as u64) < cookie {
                continue;
            }
            let bytes = name.as_bytes();
            let size = 24usize.saturating_add(bytes.len());
            if used.saturating_add(size) > len {
                break;
            }
            let mut head = [0u8; 24];
            head[0..8].copy_from_slice(&(index as u64 + 1).to_le_bytes());
            head[16..20].copy_from_slice(&(bytes.len() as u32).to_le_bytes());
            head[20] = if is_dir { 3 } else { 4 };
            memory.write(plus(buf, used as u32)?, &head)?;
            memory.write(plus(buf, (used + 24) as u32)?, bytes)?;
            used += size;
        }
        put32(memory, u32arg(args, 4)?, used as u32)?;
        Ok(0)
    }

    fn file_len(&self, path: &str) -> usize {
        self.overlay
            .snapshot()
            .read(path)
            .map(<[u8]>::len)
            .unwrap_or(0)
    }

    fn fd_set_size(&mut self, args: &[u64], budget: &mut StepBudget) -> Result<u32> {
        let Some(Fd::File { path, write, .. }) = self.fds.get(&u32arg(args, 0)?) else {
            return Ok(BADF);
        };
        if !write || path.starts_with("/logs/") || path.starts_with("/code/") {
            return Ok(NOTCAPABLE);
        }
        let size = *args.get(1).ok_or(Error::InvalidArgument)?;
        if size > usize::MAX as u64 {
            return Ok(INVAL);
        }
        let path = path.clone();
        // Growth zero-fills host memory without any guest copy; reserve it from
        // the step's host-byte allowance before coglet-fs allocates.
        let growth = size.saturating_sub(self.file_len(&path) as u64);
        budget.charge_host_bytes(growth)?;
        Ok(fs_errno(self.overlay.truncate(&path, size as usize))
            .err()
            .unwrap_or(0))
    }

    fn path_unlink(&mut self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let path =
            match self.resolve(u32arg(args, 0)?, memory, u32arg(args, 1)?, u32arg(args, 2)?)? {
                Ok(path) => path,
                Err(errno) => return Ok(errno),
            };
        Ok(fs_errno(self.overlay.unlink(&path)).err().unwrap_or(0))
    }

    fn path_rename(&mut self, args: &[u64], memory: &mut dyn GuestMemory) -> Result<u32> {
        let from =
            match self.resolve(u32arg(args, 0)?, memory, u32arg(args, 1)?, u32arg(args, 2)?)? {
                Ok(path) => path,
                Err(errno) => return Ok(errno),
            };
        let to = match self.resolve(u32arg(args, 3)?, memory, u32arg(args, 4)?, u32arg(args, 5)?)? {
            Ok(path) => path,
            Err(errno) => return Ok(errno),
        };
        Ok(fs_errno(self.overlay.rename(&from, &to)).err().unwrap_or(0))
    }

    fn fd_renumber(&mut self, args: &[u64]) -> Result<u32> {
        let from = u32arg(args, 0)?;
        let to = u32arg(args, 1)?;
        if from <= 5 || to <= 5 {
            return Ok(NOTCAPABLE);
        }
        let Some(desc) = self.fds.remove(&from) else {
            return Ok(BADF);
        };
        self.fds.insert(to, desc);
        Ok(0)
    }

    fn require_fd(&self, fd: u32) -> Result<u32> {
        Ok(if self.fds.contains_key(&fd) { 0 } else { BADF })
    }

    fn stdio_or_file(&self, fd: u32, writing: bool) -> Result<std::result::Result<Fd, u32>> {
        Ok(match self.fds.get(&fd) {
            Some(Fd::Stdin) if !writing => Ok(Fd::Stdin),
            Some(Fd::Stdout) if writing => Ok(Fd::Stdout),
            Some(Fd::Stderr) if writing => Ok(Fd::Stderr),
            Some(desc @ Fd::File { .. }) => Ok(desc.clone()),
            Some(_) => Err(NOTCAPABLE),
            None => Err(BADF),
        })
    }

    fn resolve(
        &self,
        fd: u32,
        memory: &dyn GuestMemory,
        ptr: u32,
        len: u32,
    ) -> Result<std::result::Result<String, u32>> {
        let Some(Fd::Preopen(root)) = self.fds.get(&fd) else {
            return Ok(Err(BADF));
        };
        let raw = memory.read(ptr, len)?;
        let Ok(relative) = std::str::from_utf8(&raw) else {
            return Ok(Err(NOTCAPABLE));
        };
        let path = format!("{root}/{relative}");
        Ok(if coglet_fs::validate_path(&path).is_ok() {
            Ok(path)
        } else {
            Err(NOTCAPABLE)
        })
    }
}

fn open_allowed(path: &str, mutate: bool, log_append: bool) -> bool {
    if !mutate {
        return coglet_fs::validate_path(path).is_ok();
    }
    if path.starts_with("/data/") {
        return !matches!(path, coglet_fs::INBOX | coglet_fs::HEARD);
    }
    path.starts_with("/logs/") && log_append && path != coglet_fs::EVENTS
}

fn filestat(filetype: u8, size: u64) -> [u8; 64] {
    let mut stat = [0u8; 64];
    stat[16] = filetype;
    stat[32..40].copy_from_slice(&size.to_le_bytes());
    stat
}

fn fs_code(error: coglet_fs::Error) -> u32 {
    match error {
        coglet_fs::Error::PermissionDenied => NOTCAPABLE,
        coglet_fs::Error::NotFound => NOENT,
        coglet_fs::Error::Conflict => EXIST,
        coglet_fs::Error::InvalidArgument => INVAL,
        coglet_fs::Error::ResourceExhausted => NOSPC,
    }
}

fn fs_errno<T>(result: coglet_fs::Result<T>) -> std::result::Result<T, u32> {
    result.map_err(fs_code)
}

fn fs_string(error: coglet_fs::Error) -> String {
    error.to_string()
}

fn u32arg(args: &[u64], index: usize) -> Result<u32> {
    u32::try_from(*args.get(index).ok_or(Error::InvalidArgument)?)
        .map_err(|_| Error::InvalidPointer)
}

fn put32(memory: &mut dyn GuestMemory, ptr: u32, value: u32) -> Result<()> {
    memory.write(ptr, &value.to_le_bytes())
}

fn put64(memory: &mut dyn GuestMemory, ptr: u32, value: u64) -> Result<()> {
    memory.write(ptr, &value.to_le_bytes())
}

fn get32(memory: &dyn GuestMemory, ptr: u32) -> Result<u32> {
    Ok(u32::from_le_bytes(
        memory
            .read(ptr, 4)?
            .try_into()
            .map_err(|_| Error::InvalidPointer)?,
    ))
}

fn plus(ptr: u32, n: u32) -> Result<u32> {
    ptr.checked_add(n).ok_or(Error::InvalidPointer)
}
