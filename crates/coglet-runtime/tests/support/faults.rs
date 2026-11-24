//! Test-only message-boundary faults. QUIC packets are not reordered here:
//! independent operation streams / adapter messages are scheduled explicitly.
use std::collections::BTreeMap;

pub const SEED: u64 = 0x00C0_61E7;
pub const MAX_ATTEMPTS: usize = 3;

#[derive(Clone, Copy, Debug)]
pub enum Fault {
    Pass,
    Drop,
    Delay(u64),
    Disconnect,
}

pub struct Link<T> {
    pending: BTreeMap<(u64, u64), T>,
    serial: u64,
    seed: u64,
    connected: bool,
    pub attempts: usize,
}
impl<T> Link<T> {
    pub fn new() -> Self {
        Self {
            pending: BTreeMap::new(),
            serial: 0,
            seed: SEED,
            connected: true,
            attempts: 0,
        }
    }
    /// A bounded scenario driver, not a production retry policy. Callers retain
    /// the original immutable request and decide whether retry is authorized.
    pub fn send(&mut self, now: u64, value: T, fault: Fault) -> bool {
        assert!(
            self.attempts < MAX_ATTEMPTS,
            "scenario retry budget exhausted"
        );
        self.attempts += 1;
        if matches!(fault, Fault::Disconnect) {
            self.connected = false;
            self.pending.clear();
        }
        if !self.connected || matches!(fault, Fault::Drop) {
            return false;
        }
        let delay = match fault {
            Fault::Delay(d) => d,
            _ => 0,
        };
        self.serial += 1;
        // Fixed-seed tie ordering makes schedules portable without wall sleeps.
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        assert!(
            self.pending
                .insert(
                    (now.checked_add(delay).unwrap(), self.seed ^ self.serial),
                    value
                )
                .is_none()
        );
        true
    }
    pub fn reconnect(&mut self) {
        self.connected = true;
    }
    pub fn drain(&mut self, now: u64) -> Vec<T> {
        let mut ready = Vec::new();
        while self
            .pending
            .first_key_value()
            .is_some_and(|(key, _)| key.0 <= now)
        {
            ready.push(self.pending.pop_first().unwrap().1);
        }
        ready
    }
}
