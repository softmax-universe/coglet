//! Bounded Git validation, deliberately separate from authority/store/Session.
//!
//! Install `coglet-git-validator` alongside the service and configure its trusted
//! absolute path. Linux is the initial resource-enforced implementation; other
//! platforms (including Darwin) refuse subprocess execution with
//! `ValidationError::Unavailable` before spawning anything. Linux uses
//! `waitid(WNOWAIT)` to retain the leader PID/process group until group kill;
//! there is no reaping fallback on unsupported platforms. Porting exit detection
//! alone is insufficient: the helper also requires Linux resource enforcement.
//! Git never imports the attacker pack: the helper first
//! expands it with allocation/depth fences, writes precharged loose objects, and
//! runs only read-only Git commands. No working tree or accepted ref is created.
use crate::{git_metadata::VerifiedRef, quarantine::BundleEnvelope};
use serde::{Deserialize, Serialize};
use std::{
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};
mod pack;
mod tree;

#[derive(Debug, thiserror::Error)]
pub enum ValidationError {
    #[error("invalid Git bundle: {0}")]
    Invalid(String),
    #[error("resource_exhausted: {0}")]
    Resource(String),
    #[error("validator unavailable: {0}")]
    Unavailable(String),
    #[error(transparent)]
    Io(#[from] io::Error),
}
type Result<T> = std::result::Result<T, ValidationError>;
fn invalid(message: &str) -> ValidationError {
    ValidationError::Invalid(message.into())
}
fn resource(message: &str) -> ValidationError {
    ValidationError::Resource(message.into())
}

/// All bounds are local receiver policy, never remote suggestions. Expanded
/// bytes charge delta programs AND reconstructed objects, and archive output.
/// CPU seconds are an aggregate ceiling split equally between the helper,
/// fsck and archive. Sub-second remainder is deliberately unused.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Limits {
    pub expanded_bytes: u64,
    pub objects: u32,
    pub delta_depth: u32,
    pub disk_bytes: u64,
    pub cpu_seconds: u64,
    pub wall_millis: u64,
}
impl Default for Limits {
    fn default() -> Self {
        Self {
            expanded_bytes: 256 * 1024 * 1024,
            objects: 100000,
            delta_depth: 50,
            disk_bytes: 256 * 1024 * 1024,
            cpu_seconds: 30,
            wall_millis: 30000,
        }
    }
}
impl Limits {
    fn check(self) -> Result<()> {
        if self.expanded_bytes == 0
            || self.expanded_bytes > 256 * 1024 * 1024
            || self.objects == 0
            || self.objects > 100000
            || self.delta_depth > 50
            || self.disk_bytes == 0
            || self.disk_bytes > 256 * 1024 * 1024
            || !(3..=30).contains(&self.cpu_seconds)
            || !(1..=30000).contains(&self.wall_millis)
        {
            return Err(invalid("invalid local limits"));
        }
        Ok(())
    }
}

/// Trusted helper installation path, not a remote-selected executable.
pub struct Validator {
    helper: PathBuf,
}
impl Validator {
    pub fn new(helper: impl AsRef<Path>) -> Self {
        Self {
            helper: helper.as_ref().into(),
        }
    }
}

/// An immutable, privately held verified repository and original envelope.
/// Drop removes both. This type cannot activate code or mutate accepted refs.
/// Provenance validity is structural/output binding, NOT build qualification.
pub struct VerifiedBundle {
    envelope: BundleEnvelope,
}
impl VerifiedBundle {
    pub fn metadata(&self) -> &VerifiedRef {
        self.envelope.metadata()
    }
    /// Crate-private copy while exclusive validation custody is still held.
    pub(crate) fn retained_bundle(&self) -> io::Result<Vec<u8>> {
        fs::read(self.envelope.directory()?.join("bundle"))
    }
}
struct Cancel(Arc<AtomicBool>);
impl Drop for Cancel {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

pub(crate) async fn validate(
    envelope: BundleEnvelope,
    validator: &Validator,
    limits: Limits,
) -> Result<VerifiedBundle> {
    limits.check()?;
    if !cfg!(target_os = "linux") || !validator.helper.is_absolute() {
        return Err(ValidationError::Unavailable(
            "Linux and an absolute helper path required".into(),
        ));
    }
    let helper = validator.helper.clone();
    let cancelled = Arc::new(AtomicBool::new(false));
    let guard = Cancel(cancelled.clone());
    let result = tokio::task::spawn_blocking(move || {
        let outcome = (|| {
            if envelope.object_count() > limits.objects {
                return Err(resource("object count"));
            }
            let directory = envelope.directory()?.to_owned();
            // Reserve existing bundle and lease blocks before starting ANY writes.
            // The helper maintains the same ledger for every file/directory it adds.
            let job = Job {
                tip: envelope.metadata().new_tip().into(),
                archive: envelope.metadata().archive_sha256().into(),
                reference: envelope.metadata().signed()["statement"]["ref"]
                    .as_str()
                    .expect("verified ref")
                    .into(),
                limits,
            };
            let mut command = limited_command(&helper, limits.cpu_seconds / 3);
            command
                .arg(&directory)
                .arg(serde_json::to_string(&job).map_err(|_| invalid("job encoding"))?)
                .stdin(Stdio::from(envelope.lease()?))
                .stdout(Stdio::piped())
                .stderr(Stdio::null());
            #[cfg(unix)]
            {
                use std::os::unix::process::CommandExt;
                command.process_group(0);
            }
            let start = Instant::now();
            let mut child = command
                .spawn()
                .map_err(|e| ValidationError::Unavailable(e.to_string()))?;
            // Command retains its configured Stdio after spawn. Close that extra
            // lease now, not after the result has been delivered to the async caller.
            drop(command);
            loop {
                if cancelled.load(Ordering::Acquire)
                    || start.elapsed() >= Duration::from_millis(limits.wall_millis)
                {
                    kill_group(child.id());
                    let _ = child.wait();
                    return Err(resource("validation wall time/cancelled"));
                }
                // Keep the leader unreaped until group termination: its PID/PGID
                // cannot be recycled and redirect our kill at an unrelated job.
                match leader_exited_unreaped(child.id()) {
                    Ok(false) => std::thread::sleep(Duration::from_millis(2)),
                    Ok(true) => break,
                    Err(error) => {
                        kill_group(child.id());
                        let _ = child.wait();
                        return Err(io::Error::other(error).into());
                    }
                }
            }
            // Lease inheritance protects deferred cleanup even if a descendant is
            // still in uninterruptible I/O after SIGKILL.
            kill_group(child.id());
            let status = child.wait()?;
            let mut message = String::new();
            if let Some(stdout) = child.stdout.take() {
                stdout.take(1024).read_to_string(&mut message)?;
            }
            match status.code() {
                Some(0) if message == "verified\n" => Ok(()),
                Some(20) => Err(invalid(message.trim())),
                Some(21) => Err(resource(message.trim())),
                Some(22) => Err(ValidationError::Unavailable(message.trim().into())),
                _ => Err(resource("helper CPU/memory/process limit")),
            }
        })();
        match outcome {
            Ok(()) => Ok(VerifiedBundle { envelope }),
            Err(error) => {
                // Drop captured custody before publishing the failed worker
                // result; don't depend on closure-environment drop ordering.
                drop(envelope);
                Err(error)
            }
        }
    })
    .await
    .map_err(|e| ValidationError::Unavailable(e.to_string()))?;
    drop(guard);
    result
}

// This observation must never consume the child's exit status. The caller
// kills the retained process group before its sole reaping wait.
#[cfg(target_os = "linux")]
fn leader_exited_unreaped(pid: u32) -> io::Result<bool> {
    use nix::{
        sys::wait::{Id, WaitPidFlag, WaitStatus, waitid},
        unistd::Pid,
    };
    waitid(
        Id::Pid(Pid::from_raw(pid as i32)),
        WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
    )
    .map(|status| !matches!(status, WaitStatus::StillAlive))
    .map_err(io::Error::other)
}

#[cfg(not(target_os = "linux"))]
fn leader_exited_unreaped(_pid: u32) -> io::Result<bool> {
    // validate() refuses before spawn; defense in depth, never a reaping poll.
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "non-reaping validator supervision requires Linux",
    ))
}

fn kill_group(pid: u32) {
    #[cfg(unix)]
    {
        use nix::{
            sys::signal::{Signal, killpg},
            unistd::Pid,
        };
        let _ = killpg(Pid::from_raw(pid as i32), Signal::SIGKILL);
    }
}
fn limited_command(executable: &Path, seconds: u64) -> Command {
    let mut command = Command::new("/usr/bin/prlimit");
    command
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("LC_ALL", "C")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_ATTR_NOSYSTEM", "1")
        .env("GIT_NO_REPLACE_OBJECTS", "1")
        .arg(format!("--cpu={seconds}:{seconds}"))
        .arg("--as=1073741824:1073741824")
        .arg("--core=0:0")
        .arg("--nofile=64:64")
        .arg("--")
        .arg(executable)
        .stdin(Stdio::null());
    command
}

#[cfg(all(test, target_os = "linux"))]
mod supervision_tests {
    use super::*;
    use nix::{
        errno::Errno,
        sys::wait::{Id, WaitPidFlag, WaitStatus, waitid},
        unistd::Pid,
    };
    use std::os::unix::process::CommandExt;

    #[test]
    fn leader_is_unreaped_until_group_kill() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 7"])
            .process_group(0)
            .spawn()
            .unwrap();
        let pid = Pid::from_raw(child.id() as i32);
        let deadline = Instant::now() + Duration::from_secs(5);
        while !leader_exited_unreaped(child.id()).unwrap() {
            if Instant::now() >= deadline {
                kill_group(child.id());
                let _ = child.wait();
                panic!("leader did not exit");
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        // Independent observation fails with ECHILD if the production poll
        // reaped. Repeated observations must preserve the same exit status.
        let observe = || {
            waitid(
                Id::Pid(pid),
                WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
            )
        };
        assert_eq!(observe().unwrap(), WaitStatus::Exited(pid, 7));
        assert!(leader_exited_unreaped(child.id()).unwrap());
        assert_eq!(observe().unwrap(), WaitStatus::Exited(pid, 7));
        // The retained zombie pins the PGID, even with no living members.
        nix::sys::signal::killpg(pid, None).unwrap();
        kill_group(child.id());
        assert_eq!(observe().unwrap(), WaitStatus::Exited(pid, 7));
        assert_eq!(child.wait().unwrap().code(), Some(7));
        assert_eq!(observe(), Err(Errno::ECHILD));
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Job {
    tip: String,
    archive: String,
    reference: String,
    limits: Limits,
}

/// Entry point for the separately installed helper binary. Not an admission API.
/// All verdicts originate in this trusted executable, never in Git output.
#[doc(hidden)]
pub fn helper_main() -> i32 {
    let outcome = (|| {
        if !cfg!(target_os = "linux") {
            return Err(ValidationError::Unavailable("Linux required".into()));
        }
        let args: Vec<_> = std::env::args_os().skip(1).collect();
        if args.len() != 2 || args[1].len() > 2048 {
            return Err(invalid("helper arguments"));
        }
        let directory = PathBuf::from(&args[0]);
        let job: Job = serde_json::from_str(
            args[1]
                .to_str()
                .ok_or_else(|| invalid("helper arguments"))?,
        )
        .map_err(|_| invalid("helper job"))?;
        job.limits.check()?;
        if job.tip.len() != 40
            || !job
                .tip
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
            || !matches!(
                job.reference.as_str(),
                "refs/heads/logs" | "refs/heads/hub" | "refs/published/main"
            )
        {
            return Err(invalid("helper metadata"));
        }
        run(&directory, &job)
    })();
    match outcome {
        Ok(()) => {
            println!("verified");
            0
        }
        Err(error) => {
            let code = match error {
                ValidationError::Resource(_) => 21,
                ValidationError::Unavailable(_) => 22,
                _ => 20,
            };
            println!(
                "{}",
                error.to_string().chars().take(512).collect::<String>()
            );
            code
        }
    }
}

/// A pre-write disk reservation ledger. Charge allocated 4KiB blocks plus a
/// block per directory, not just logical payload size. Refuse filesystems whose
/// allocation units exceed that conservative unit (checked with statvfs).
struct Disk {
    used: u64,
    limit: u64,
}
impl Disk {
    fn charge(&mut self, bytes: u64) -> Result<()> {
        let charge = bytes
            .max(1)
            .div_ceil(4096)
            .checked_mul(4096)
            .ok_or_else(|| resource("disk"))?;
        self.used = self
            .used
            .checked_add(charge)
            .filter(|n| *n <= self.limit)
            .ok_or_else(|| resource("disk"))?;
        Ok(())
    }
    fn directory(&mut self, path: &Path) -> Result<()> {
        self.charge(4096)?;
        fs::create_dir(path)?;
        Ok(())
    }
    fn write(&mut self, path: &Path, bytes: &[u8]) -> Result<()> {
        use std::io::Write;
        // Reserve a directory growth/index block as well as file data before
        // creation. This deliberately overcharges tiny files and fanout growth.
        self.charge(4096)?;
        self.charge(bytes.len() as u64)?;
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        Ok(())
    }
}
fn run(directory: &Path, job: &Job) -> Result<()> {
    let stat = nix::sys::statvfs::statvfs(directory).map_err(io::Error::other)?;
    if stat.block_size() > 4096 || stat.fragment_size() > 4096 {
        return Err(ValidationError::Unavailable(
            "filesystem allocation unit exceeds disk ledger".into(),
        ));
    }
    let bundle = directory.join("bundle");
    let size = fs::metadata(&bundle)?.len();
    if size > 67108864 {
        return Err(resource("compressed bundle"));
    }
    let mut disk = Disk {
        used: 0,
        limit: job.limits.disk_bytes,
    };
    disk.charge(size)?;
    disk.charge(4096)?;
    disk.charge(4096)?; // bundle, lease, custody directory
    let raw = fs::read(bundle)?;
    let header = format!("# v2 git bundle\n{} {}\n\n", job.tip, job.reference);
    if !raw.starts_with(header.as_bytes()) {
        return Err(invalid("bundle header"));
    }
    let objects = pack::decode(&raw[header.len()..], job.limits)?;
    tree::check(&objects, job)?;
    let repo = directory.join("repo");
    disk.directory(&repo)?;
    disk.directory(&repo.join("objects"))?;
    disk.directory(&repo.join("refs"))?;
    disk.write(&repo.join("HEAD"), b"ref: refs/heads/unborn\n")?;
    disk.write(
        &repo.join("config"),
        b"[core]\nrepositoryformatversion = 0\nbare = true\nhooksPath = /dev/null\n",
    )?;
    use flate2::{Compression, write::ZlibEncoder};
    use std::io::Write;
    let mut dirs = std::collections::HashSet::new();
    for object in objects.values() {
        let name = hex::encode(object.oid);
        if dirs.insert(name[..2].to_owned()) {
            disk.directory(&repo.join("objects").join(&name[..2]))?;
        }
        let mut encoded = ZlibEncoder::new(Vec::new(), Compression::fast());
        write!(encoded, "{} {}\0", object.kind, object.bytes.len())?;
        encoded.write_all(&object.bytes)?;
        let encoded = encoded.finish()?;
        disk.write(
            &repo.join("objects").join(&name[..2]).join(&name[2..]),
            &encoded,
        )?;
    }
    // No importer, refs, hooks, alternates, replace refs, filters, or attributes.
    let mut fsck = git(&repo, job.limits.cpu_seconds / 3);
    let status = fsck
        .args([
            "fsck",
            "--full",
            "--strict",
            "--no-reflogs",
            "--no-dangling",
            &job.tip,
        ])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()?;
    if !status.success() {
        return Err(if status.code().is_none() {
            resource("fsck CPU")
        } else {
            invalid("strict fsck")
        });
    }
    let mut archive = git(&repo, job.limits.cpu_seconds / 3);
    let mut child = archive
        .args(["archive", "--format=tar", &job.tip])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()?;
    use sha2::{Digest, Sha256};
    let result = (|| {
        let mut hash = Sha256::new();
        let mut count = 0u64;
        let mut buffer = [0; 8192];
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| invalid("archive output"))?;
        loop {
            let n = stdout.read(&mut buffer)?;
            if n == 0 {
                break;
            }
            count = count
                .checked_add(n as u64)
                .filter(|n| *n <= job.limits.expanded_bytes)
                .ok_or_else(|| resource("archive bytes"))?;
            hash.update(&buffer[..n]);
        }
        if hex::encode(hash.finalize()) != job.archive {
            return Err(invalid("archive digest"));
        }
        Ok(())
    })();
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    result?;
    if !status.success() {
        return Err(if status.code().is_none() {
            resource("archive CPU")
        } else {
            invalid("archive")
        });
    }
    // Persist all newly created directory entries before handing private custody
    // back to the parent. These are known paths, not an untrusted recursive walk.
    for prefix in dirs {
        fs::File::open(repo.join("objects").join(prefix))?.sync_all()?;
    }
    fs::File::open(repo.join("objects"))?.sync_all()?;
    fs::File::open(repo.join("refs"))?.sync_all()?;
    fs::File::open(&repo)?.sync_all()?;
    fs::File::open(directory)?.sync_all()?;
    Ok(())
}
fn git(repo: &Path, cpu: u64) -> Command {
    let mut command = limited_command(Path::new("/usr/bin/git"), cpu);
    command
        .current_dir(repo)
        .arg("--no-replace-objects")
        .arg("-c")
        .arg("core.attributesFile=/dev/null")
        .arg("-c")
        .arg("core.hooksPath=/dev/null")
        .arg("-c")
        .arg("protocol.allow=never");
    // stdin is an inherited locked lease, never bundle input. Duplicate the
    // same open description so Git retains custody if its supervisor crashes.
    use std::os::fd::AsFd;
    match std::io::stdin().as_fd().try_clone_to_owned() {
        Ok(fd) => {
            command.stdin(Stdio::from(fs::File::from(fd)));
        }
        Err(_) => {
            command.arg("--invalid-lease");
        } // fail closed
    }
    command
}
