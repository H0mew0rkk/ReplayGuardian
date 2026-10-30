use serde_json::Value;
use std::time::{Duration, Instant};

// This is our conservative pacing policy, not a claimed NVIDIA server quota.
pub const TOOL_GAP: Duration = Duration::from_secs(5);
pub const MESSAGE: &str = "NVIDIA MCP 暂时限流";
pub struct Gate {
    next_tool: Option<Instant>,
    blocked_until: Option<Instant>,
    tool_gap: Duration,
}
impl Default for Gate {
    fn default() -> Self {
        Self {
            next_tool: None,
            blocked_until: None,
            tool_gap: TOOL_GAP,
        }
    }
}
impl Gate {
    pub fn acquire(&mut self, now: Instant, tool: bool) -> Result<(), Duration> {
        let deadline = if tool {
            self.next_tool.into_iter().chain(self.blocked_until).max()
        } else {
            self.blocked_until
        };
        if let Some(deadline) = deadline
            && deadline > now
        {
            return Err(deadline.duration_since(now));
        }
        if tool {
            self.next_tool = Some(now + self.tool_gap);
        }
        Ok(())
    }
    pub fn block(&mut self, now: Instant, delay: Duration) {
        self.tool_gap = (self.tool_gap + delay).min(Duration::from_secs(60));
        let deadline = now + delay + Duration::from_millis(250);
        self.blocked_until = Some(self.blocked_until.map_or(deadline, |old| old.max(deadline)));
    }
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.blocked_until
            .filter(|deadline| *deadline > now)
            .map(|deadline| deadline.duration_since(now))
    }
    pub fn tool_gap(&self) -> Duration {
        self.tool_gap
    }
}
pub fn retry_after(value: &Value) -> Option<Duration> {
    if value["error"]["code"].as_i64() != Some(-32029) {
        return None;
    }
    let seconds = value["error"]["data"]["retryAfter"]
        .as_f64()
        .filter(|v| v.is_finite() && *v >= 0.0 && *v <= 86400.0)
        .unwrap_or(30.0);
    Some(Duration::from_secs_f64(seconds.max(1.0)))
}
pub fn message(delay: Duration) -> String {
    format!(
        "{MESSAGE}，等待至少 {} 秒后再读取状态",
        delay.as_secs_f64().ceil() as u64
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn repeated_server_limits_reduce_client_frequency() {
        let now = Instant::now();
        let mut gate = Gate::default();
        gate.block(now, Duration::from_secs(9));
        assert!(gate.tool_gap() > TOOL_GAP);
        for _ in 0..20 {
            gate.block(now, Duration::from_secs(9));
        }
        assert_eq!(gate.tool_gap(), Duration::from_secs(60));
    }
    #[test]
    fn parses_server_delays() {
        for n in [1, 2, 7, 9] {
            assert_eq!(
                retry_after(&json!({"error":{"code":-32029,"data":{"retryAfter":n}}})),
                Some(Duration::from_secs(n))
            );
        }
    }
    #[test]
    fn malformed_hint_uses_safe_default() {
        assert_eq!(
            retry_after(&json!({"error":{"code":-32029,"data":{"retryAfter":"bad"}}})),
            Some(Duration::from_secs(30))
        );
        assert_eq!(retry_after(&json!({"error":{"code":-32601}})), None);
    }
    #[test]
    fn serial_tools_are_spaced_but_discovery_not_charged() {
        let now = Instant::now();
        let mut gate = Gate::default();
        assert!(gate.acquire(now, true).is_ok());
        assert_eq!(gate.acquire(now, true), Err(TOOL_GAP));
        assert!(gate.acquire(now, false).is_ok());
        assert!(gate.acquire(now + TOOL_GAP, true).is_ok());
    }
    #[test]
    fn server_cooldown_applies_to_all_calls_and_is_not_shortened() {
        let now = Instant::now();
        let mut gate = Gate::default();
        gate.block(now, Duration::from_secs(9));
        gate.block(now, Duration::from_secs(2));
        assert!(gate.acquire(now + Duration::from_secs(8), false).is_err());
        assert!(gate.acquire(now + Duration::from_secs(9), true).is_err());
        assert!(gate.acquire(now + Duration::from_secs(10), true).is_ok());
    }
}
