//! Private, Linux-only supervisor for fixed trusted Git commands.
use crate::{Error, Result};
use std::{process::Command, time::Duration};
#[cfg(target_os = "linux")]
pub(super) fn run(mut command: Command, max: usize, wall: Duration) -> Result<Vec<u8>> {
    use nix::{
        sys::{
            resource::{Resource, setrlimit},
            signal::{Signal, killpg},
            wait::{Id, WaitPidFlag, WaitStatus, waitid},
        },
        unistd::Pid,
    };
    use std::{io::Read, os::unix::process::CommandExt, sync::mpsc, time::Instant};
    command.process_group(0);
    // Only async-signal-safe setrlimit calls between fork and exec.
    unsafe {
        command.pre_exec(|| {
            setrlimit(Resource::RLIMIT_CPU, 5, 5).map_err(std::io::Error::other)?;
            setrlimit(Resource::RLIMIT_AS, 512 * 1024 * 1024, 512 * 1024 * 1024)
                .map_err(std::io::Error::other)?;
            setrlimit(Resource::RLIMIT_FSIZE, 64 * 1024 * 1024, 64 * 1024 * 1024)
                .map_err(std::io::Error::other)?;
            Ok(())
        });
    }
    let mut child = command.spawn().map_err(|e| {
        eprintln!("build supervisor spawn: {e:?}");
        Error::Invalid
    })?;
    drop(command);
    let pid = Pid::from_raw(child.id() as i32);
    let stdout = child.stdout.take().ok_or(Error::Invalid)?;
    let (send, recv) = mpsc::sync_channel(1);
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let result = stdout
            .take(max as u64 + 1)
            .read_to_end(&mut out)
            .map(|_| out);
        let _ = send.send(result);
    });
    let start = Instant::now();
    let mut output = None;
    let mut failed = false;
    loop {
        if output.is_none()
            && let Ok(result) = recv.try_recv()
        {
            output = Some(result);
        }
        if output
            .as_ref()
            .is_some_and(|r| r.as_ref().map_or(true, |b| b.len() > max))
            || start.elapsed() >= wall
        {
            failed = true;
            break;
        }
        match waitid(
            Id::Pid(pid),
            WaitPidFlag::WEXITED | WaitPidFlag::WNOHANG | WaitPidFlag::WNOWAIT,
        ) {
            Ok(WaitStatus::StillAlive) => std::thread::sleep(Duration::from_millis(2)),
            Ok(_) => break,
            Err(_) => {
                failed = true;
                break;
            }
        }
    }
    // Never reap before killpg: leader PID/PGID remains reserved until the sole
    // wait below, so cleanup cannot target a recycled unrelated process group.
    let _ = killpg(pid, Signal::SIGKILL);
    let status = child.wait();
    let _ = reader.join();
    let result = output
        .or_else(|| recv.try_recv().ok())
        .ok_or(Error::Invalid)?
        .map_err(|_| Error::Invalid)?;
    if failed || result.len() > max || !status.map_err(|_| Error::Invalid)?.success() {
        return Err(Error::Full);
    }
    Ok(result)
}
#[cfg(not(target_os = "linux"))]
pub(super) fn run(_: Command, _: usize, _: Duration) -> Result<Vec<u8>> {
    Err(Error::Denied)
}
#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        process::{Command, Stdio},
        time::{Duration, Instant},
    };
    #[test]
    fn git_supervisor_kills_hung_process_with_original_group_custody() {
        let mut cmd = Command::new("/bin/sleep");
        cmd.arg("20").stdout(Stdio::piped()).stderr(Stdio::null());
        let start = Instant::now();
        assert!(run(cmd, 1024, Duration::from_millis(30)).is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
    }
    #[test]
    fn git_supervisor_refuses_overflow_without_unbounded_capture() {
        let mut cmd = Command::new("/usr/bin/yes");
        cmd.stdout(Stdio::piped()).stderr(Stdio::null());
        assert!(run(cmd, 1024, Duration::from_secs(2)).is_err());
    }
}
