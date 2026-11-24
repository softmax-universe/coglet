//! Bounded local child supervisor. No inherited credentials or unbounded pipes.
use anyhow::{Context, Result, bail, ensure};
use std::{
    io::{Read, Write},
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};
struct Guard(Child, bool);
impl Guard {
    fn stop(&mut self) {
        if !self.1 {
            return;
        }
        self.1 = false;
        #[cfg(unix)]
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Drop for Guard {
    fn drop(&mut self) {
        self.stop();
    }
}
fn pipe(
    mut pipe: impl Read + Send + 'static,
    limit: usize,
    tx: mpsc::Sender<bool>,
) -> std::thread::JoinHandle<std::io::Result<Vec<u8>>> {
    std::thread::spawn(move || {
        let mut raw = Vec::new();
        let result = pipe.by_ref().take(limit as u64 + 1).read_to_end(&mut raw);
        let _ = tx.send(result.is_ok() && raw.len() <= limit);
        result?;
        Ok(raw)
    })
}
pub fn run(
    mut command: Command,
    input: Vec<u8>,
    timeout: Duration,
    max_stdout: usize,
) -> Result<Vec<u8>> {
    command.env_clear();
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = Guard(
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("start cogletd (build with native-engine)")?,
        true,
    );
    let mut stdin = child.0.stdin.take().context("child stdin")?;
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let (tx, rx) = mpsc::channel();
    let stdout = pipe(
        child.0.stdout.take().context("child stdout")?,
        max_stdout,
        tx.clone(),
    );
    let stderr = pipe(child.0.stderr.take().context("child stderr")?, 16384, tx);
    let started = Instant::now();
    let mut finished = 0;
    let mut failure = None;
    let mut status = None;
    loop {
        while let Ok(valid) = rx.try_recv() {
            finished += 1;
            if !valid {
                failure = Some("cogletd output limit/read failure");
            }
        }
        if failure.is_some() {
            break;
        }
        if started.elapsed() >= timeout {
            failure = Some("cogletd supervisor timeout");
            break;
        }
        status = child.0.try_wait()?;
        if status.is_some() && finished == 2 {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    // Terminate the whole owned process group even if its leader exited while
    // a descendant retained a pipe. Never leave a child alive on any return path.
    child.stop();
    let written = writer
        .join()
        .map_err(|_| anyhow::anyhow!("child input thread panicked"))?;
    let stdout = stdout
        .join()
        .map_err(|_| anyhow::anyhow!("child stdout thread panicked"))??;
    let stderr = stderr
        .join()
        .map_err(|_| anyhow::anyhow!("child stderr thread panicked"))??;
    if let Some(failure) = failure {
        bail!(failure);
    }
    ensure!(
        status.is_some_and(|s| s.success()),
        "cogletd failed: {}",
        String::from_utf8_lossy(&stderr)
            .chars()
            .filter(|c| !c.is_control() || *c == '\n')
            .take(2048)
            .collect::<String>()
    );
    written?;
    Ok(stdout)
}
