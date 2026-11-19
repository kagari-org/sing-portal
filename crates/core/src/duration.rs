use std::time::Duration;

/// Parses `30`, `15s`, `6h` or `2d`. Shared by both binaries so their flags
/// accept the same shapes.
pub fn parse_duration(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    if value.is_empty() {
        return Err("duration is empty".into());
    }
    if let Ok(seconds) = value.parse::<u64>() {
        return Ok(Duration::from_secs(seconds));
    }
    let (number, unit) = value.split_at(value.len() - 1);
    let seconds = number
        .parse::<u64>()
        .map_err(|_| format!("invalid duration `{value}`"))?;
    match unit {
        "s" => Ok(Duration::from_secs(seconds)),
        "m" => Ok(Duration::from_secs(seconds * 60)),
        "h" => Ok(Duration::from_secs(seconds * 3600)),
        "d" => Ok(Duration::from_secs(seconds * 86400)),
        other => Err(format!("unknown duration unit `{other}`")),
    }
}
