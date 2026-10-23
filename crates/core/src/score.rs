#[derive(Debug, Clone, Copy)]
pub struct ScoreConfig {
    pub latency_ref_ms: f64,
    pub bandwidth_ref_mbps: f64,
    pub bandwidth_floor: f64,
}

impl Default for ScoreConfig {
    fn default() -> Self {
        Self {
            latency_ref_ms: 60.0,
            bandwidth_ref_mbps: 20.0,
            bandwidth_floor: 0.5,
        }
    }
}

impl ScoreConfig {
    pub fn latency_score(&self, latency_ms: f64) -> f64 {
        if !latency_ms.is_finite() || latency_ms <= 0.0 {
            return 0.0;
        }
        (self.latency_ref_ms / latency_ms).min(1.0)
    }

    pub fn bandwidth_score(&self, down_mbps: Option<f64>) -> f64 {
        let floor = self.bandwidth_floor.clamp(0.0, 1.0);
        let Some(mbps) = down_mbps.filter(|mbps| mbps.is_finite() && *mbps > 0.0) else {
            return floor;
        };
        let reference = self.bandwidth_ref_mbps.max(1.0);
        let ratio = (1.0 + mbps).ln() / (1.0 + reference).ln();
        floor + (1.0 - floor) * ratio.min(1.0)
    }

    pub fn score(&self, latency_ms: f64, down_mbps: Option<f64>) -> f64 {
        let latency = self.latency_score(latency_ms);
        if latency <= 0.0 {
            return 0.0;
        }
        100.0 * (latency * self.bandwidth_score(down_mbps)).sqrt()
    }
}

pub fn should_switch(best: f64, current: f64, margin: f64) -> bool {
    if !best.is_finite() || best <= 0.0 {
        return false;
    }
    if !current.is_finite() || current <= 0.0 {
        return true;
    }
    best >= current * (1.0 + margin.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(left: f64, right: f64) {
        assert!(
            (left - right).abs() < 0.15,
            "expected {right}, got {left}"
        );
    }

    #[test]
    fn latency_saturates_at_reference() {
        let config = ScoreConfig::default();
        close(config.latency_score(10.0), 1.0);
        close(config.latency_score(60.0), 1.0);
        close(config.latency_score(120.0), 0.5);
        close(config.latency_score(0.0), 0.0);
        close(config.latency_score(-1.0), 0.0);
    }

    #[test]
    fn bandwidth_is_soft_and_floored() {
        let config = ScoreConfig::default();
        close(config.bandwidth_score(None), 0.5);
        close(config.bandwidth_score(Some(0.0)), 0.5);
        close(config.bandwidth_score(Some(1.2)), 0.63);
        close(config.bandwidth_score(Some(5.0)), 0.79);
        close(config.bandwidth_score(Some(20.0)), 1.0);
        close(config.bandwidth_score(Some(500.0)), 1.0);
    }

    #[test]
    fn score_keeps_weak_bandwidth_competitive() {
        let config = ScoreConfig::default();
        close(config.score(30.0, Some(5.0)), 89.1);
        close(config.score(120.0, Some(4.0)), 61.8);
        close(config.score(60.0, Some(1.2)), 79.3);
        close(config.score(400.0, Some(4.5)), 34.2);
        close(config.score(25.0, Some(100.0)), 100.0);
    }

    #[test]
    fn missing_bandwidth_is_not_a_disqualifier() {
        let config = ScoreConfig::default();
        close(config.score(30.0, None), 70.7);
        assert!(config.score(30.0, None) > 0.0);
    }

    #[test]
    fn switching_uses_relative_margin() {
        assert!(should_switch(90.0, 0.0, 0.15));
        assert!(should_switch(90.0, 70.0, 0.15));
        assert!(!should_switch(75.0, 70.0, 0.15));
        assert!(!should_switch(0.0, 70.0, 0.15));
    }
}
