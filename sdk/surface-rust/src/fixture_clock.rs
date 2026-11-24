//! Test-adapter clock binding. Never used by a production Surface connection.
//! The private socket carries uint64 microsecond advances and echoed barriers.
//! Ordinary frame sends are also acknowledged after dispatch and stdout flush.
use std::io::{Read, Write};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

#[derive(Debug)]
pub enum Input {
    Frame(Result<Option<Vec<u8>>, String>),
    Advance(u64),
    ClockError(String),
}

pub struct FixtureInput {
    receiver: Receiver<Input>,
    #[cfg(unix)]
    control: Option<std::os::unix::net::UnixStream>,
    logical: Option<u64>,
    clock: Instant,
}

impl FixtureInput {
    pub fn open() -> Result<Self, String> {
        let (sender, receiver) = mpsc::sync_channel(crate::transport::QUEUE_BOUND);
        #[cfg(unix)]
        let control = if let Some(path) = std::env::var_os("COGLET_FIXTURE_CLOCK_SOCKET") {
            let stream =
                std::os::unix::net::UnixStream::connect(path).map_err(|e| e.to_string())?;
            let mut reader = stream.try_clone().map_err(|e| e.to_string())?;
            let tx = sender.clone();
            std::thread::spawn(move || {
                loop {
                    let mut raw = [0u8; 8];
                    if let Err(e) = reader.read_exact(&mut raw) {
                        let _ = tx.send(Input::ClockError(e.to_string()));
                        break;
                    }
                    if tx.send(Input::Advance(u64::from_be_bytes(raw))).is_err() {
                        break;
                    }
                }
            });
            Some(stream)
        } else {
            None
        };
        #[cfg(not(unix))]
        if std::env::var_os("COGLET_FIXTURE_CLOCK_SOCKET").is_some() {
            return Err("fixture clock binding requires Unix".into());
        }
        std::thread::spawn(move || {
            let mut input = std::io::stdin().lock();
            loop {
                let frame = crate::transport::read_frame(&mut input).map_err(|e| e.to_string());
                let done = !matches!(&frame, Ok(Some(_)));
                if sender.send(Input::Frame(frame)).is_err() || done {
                    break;
                }
            }
        });
        #[cfg(unix)]
        let logical = control.as_ref().map(|_| 0);
        #[cfg(not(unix))]
        let logical = None;
        Ok(Self {
            receiver,
            #[cfg(unix)]
            control,
            logical,
            clock: Instant::now(),
        })
    }

    pub fn next_input(&mut self) -> Result<Option<Input>, String> {
        let result = if self.logical.is_some() {
            self.receiver
                .recv()
                .map_err(|_| "fixture input closed".to_string())?
        } else {
            match self.receiver.recv_timeout(Duration::from_millis(1)) {
                Ok(input) => input,
                Err(RecvTimeoutError::Timeout) => return Ok(None),
                Err(RecvTimeoutError::Disconnected) => return Ok(Some(Input::Frame(Ok(None)))),
            }
        };
        if let Input::Advance(now) = result {
            if self.logical.is_none_or(|previous| now < previous) {
                return Err("invalid fixture clock advance".into());
            }
            self.logical = Some(now);
        }
        Ok(Some(result))
    }

    pub fn now(&self) -> u64 {
        self.logical
            .unwrap_or_else(|| u64::try_from(self.clock.elapsed().as_micros()).unwrap_or(u64::MAX))
    }

    // Called only after dispatch/tick and stdout flush. The harness waits for
    // each send barrier before advancing, so the two input streams cannot race.
    pub fn acknowledge(&mut self) -> Result<(), String> {
        #[cfg(unix)]
        if let Some(stream) = &mut self.control {
            stream
                .write_all(&self.logical.unwrap().to_be_bytes())
                .map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn logical_clock_starts_at_zero_and_rejects_rollback() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let mut input = FixtureInput {
            receiver,
            control: None,
            logical: Some(0),
            clock: Instant::now(),
        };
        assert_eq!(input.now(), 0);
        sender.send(Input::Advance(100_000)).unwrap();
        input.next_input().unwrap();
        assert_eq!(input.now(), 100_000);
        sender.send(Input::Advance(99_999)).unwrap();
        assert!(
            input
                .next_input()
                .unwrap_err()
                .contains("invalid fixture clock")
        );
        assert_eq!(input.now(), 100_000);
    }

    #[test]
    fn ack_echoes_exact_current_microseconds() {
        let (_sender, receiver) = mpsc::sync_channel(1);
        let (control, mut other) = std::os::unix::net::UnixStream::pair().unwrap();
        other
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        let mut input = FixtureInput {
            receiver,
            control: Some(control),
            logical: Some(110_000),
            clock: Instant::now(),
        };
        input.acknowledge().unwrap();
        let mut raw = [0u8; 8];
        other.read_exact(&mut raw).unwrap();
        assert_eq!(u64::from_be_bytes(raw), 110_000);
    }
}
