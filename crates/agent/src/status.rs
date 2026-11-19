use std::sync::Mutex;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConnectionState {
    Stopped,
    Connecting,
    Connected,
    Reconnecting,
}

impl ConnectionState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stopped => "stopped",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Reconnecting => "reconnecting",
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::Stopped => 0,
            Self::Connecting => 1,
            Self::Connected => 2,
            Self::Reconnecting => 3,
        }
    }

    fn from_code(value: u8) -> Self {
        match value {
            1 => Self::Connecting,
            2 => Self::Connected,
            3 => Self::Reconnecting,
            _ => Self::Stopped,
        }
    }
}

/// What the session loop is doing, for a host that wants to show it. Only the
/// loop and the session write here; readers can poll it at any time.
pub struct RuntimeStatus {
    state: AtomicU8,
    last_connected_at: AtomicI64,
    next_retry_at: AtomicI64,
    connection_error: Mutex<String>,
}

impl RuntimeStatus {
    pub fn new() -> Self {
        Self {
            state: AtomicU8::new(ConnectionState::Stopped.code()),
            last_connected_at: AtomicI64::new(0),
            next_retry_at: AtomicI64::new(0),
            connection_error: Mutex::new(String::new()),
        }
    }

    pub fn state(&self) -> ConnectionState {
        ConnectionState::from_code(self.state.load(Ordering::Relaxed))
    }

    pub fn connected(&self) -> bool {
        self.state() == ConnectionState::Connected
    }

    pub fn set_connecting(&self) {
        self.state
            .store(ConnectionState::Connecting.code(), Ordering::Relaxed);
        self.next_retry_at.store(0, Ordering::Relaxed);
    }

    pub fn set_connected(&self) {
        self.state
            .store(ConnectionState::Connected.code(), Ordering::Relaxed);
        self.last_connected_at.store(now(), Ordering::Relaxed);
        self.next_retry_at.store(0, Ordering::Relaxed);
        self.set_error("");
    }

    pub fn set_reconnecting(&self, retry_in: Duration, error: Option<&str>) {
        self.state
            .store(ConnectionState::Reconnecting.code(), Ordering::Relaxed);
        self.next_retry_at.store(
            now().saturating_add(retry_in.as_secs() as i64),
            Ordering::Relaxed,
        );
        self.set_error(error.unwrap_or(""));
    }

    pub fn set_stopped(&self) {
        self.state
            .store(ConnectionState::Stopped.code(), Ordering::Relaxed);
        self.next_retry_at.store(0, Ordering::Relaxed);
    }

    pub fn last_connected_at(&self) -> i64 {
        self.last_connected_at.load(Ordering::Relaxed)
    }

    pub fn retry_in_secs(&self) -> i64 {
        let next = self.next_retry_at.load(Ordering::Relaxed);
        next.saturating_sub(now()).max(0)
    }

    pub fn connection_error(&self) -> String {
        self.connection_error
            .lock()
            .expect("connection error lock")
            .clone()
    }

    fn set_error(&self, message: &str) {
        *self.connection_error.lock().expect("connection error lock") = message.to_string();
    }
}

impl Default for RuntimeStatus {
    fn default() -> Self {
        Self::new()
    }
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_stopped() {
        let status = RuntimeStatus::new();
        assert_eq!(status.state(), ConnectionState::Stopped);
        assert!(!status.connected());
        assert_eq!(status.retry_in_secs(), 0);
        assert_eq!(status.last_connected_at(), 0);
        assert!(status.connection_error().is_empty());
    }

    #[test]
    fn reports_connecting() {
        let status = RuntimeStatus::new();
        status.set_connecting();
        assert_eq!(status.state(), ConnectionState::Connecting);
        assert!(!status.connected());
    }

    #[test]
    fn reports_connected() {
        let status = RuntimeStatus::new();
        status.set_connected();
        assert_eq!(status.state(), ConnectionState::Connected);
        assert!(status.connected());
        assert!(status.last_connected_at() > 0);
        assert!(status.connection_error().is_empty());
    }

    #[test]
    fn reports_reconnecting_with_retry_window() {
        let status = RuntimeStatus::new();
        status.set_reconnecting(Duration::from_secs(5), Some("boom"));
        assert_eq!(status.state(), ConnectionState::Reconnecting);
        assert!(!status.connected());
        assert!((4..=5).contains(&status.retry_in_secs()));
        assert_eq!(status.connection_error(), "boom");
    }

    #[test]
    fn reports_stopped_after_stop() {
        let status = RuntimeStatus::new();
        status.set_reconnecting(Duration::from_secs(5), Some("boom"));
        status.set_stopped();
        assert_eq!(status.state(), ConnectionState::Stopped);
        assert_eq!(status.retry_in_secs(), 0);
    }

    #[test]
    fn clearing_the_error_on_connect() {
        let status = RuntimeStatus::new();
        status.set_reconnecting(Duration::from_secs(5), Some("boom"));
        status.set_connected();
        assert!(status.connection_error().is_empty());
        assert_eq!(status.retry_in_secs(), 0);
    }
}
