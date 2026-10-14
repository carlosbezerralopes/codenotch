//! In-memory machine readings. Only presentation preferences belong in config.json.
use serde::{Deserialize, Serialize};

#[cfg(windows)]
mod windows;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub enabled: bool,
    pub refresh_seconds: u32,
    pub history_minutes: u32,
    pub order: Vec<String>,
    pub hidden: Vec<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            enabled: cfg!(windows),
            refresh_seconds: 3,
            history_minutes: 5,
            order: Vec::new(),
            hidden: Vec::new(),
        }
    }
}

impl Settings {
    pub fn validate(&self) -> Result<Self, String> {
        if ![3, 5, 10].contains(&self.refresh_seconds) {
            return Err("O intervalo deve ser 3, 5 ou 10 segundos".into());
        }
        if ![1, 5, 15].contains(&self.history_minutes) {
            return Err("O histórico deve ser 1, 5 ou 15 minutos".into());
        }
        fn ids(values: &[String]) -> Result<Vec<String>, String> {
            if values.len() > 128 {
                return Err("Seleção limitada a 128 indicadores".into());
            }
            let mut result = Vec::new();
            for id in values {
                if id.is_empty() || id.chars().count() > 160 || id.chars().any(char::is_control) {
                    return Err("Identidade de indicador inválida".into());
                }
                if !result.contains(id) {
                    result.push(id.clone());
                }
            }
            Ok(result)
        }
        Ok(Self { order: ids(&self.order)?, hidden: ids(&self.hidden)?, ..self.clone() })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Resource {
    pub id: String,
    pub kind: String,
    pub name: String,
    pub subtitle: String,
    pub vendor: Option<String>,
    pub percent: Option<f64>,
    pub value: String,
    pub detail: String,
    pub read_bps: Option<f64>,
    pub write_bps: Option<f64>,
    pub temperature_c: Option<f64>,
    pub status: String,
}

impl Resource {
    #[cfg(windows)]
    fn new(id: impl Into<String>, kind: &str, name: impl Into<String>, subtitle: impl Into<String>) -> Self {
        Self {
            id: id.into(), kind: kind.into(), name: name.into(), subtitle: subtitle.into(),
            vendor: None, percent: None, value: "—".into(), detail: "Sem leitura".into(),
            read_bps: None, write_bps: None, temperature_c: None, status: "unavailable".into(),
        }
    }

    #[cfg(windows)]
    fn warming(&mut self) {
        self.status = "warming".into();
        self.detail = "Aguardando próxima leitura".into();
    }

    #[cfg(windows)]
    fn set_percent(&mut self, value: f64) {
        self.percent = Some(value.clamp(0.0, 100.0));
        self.value = format!("{:.0}%", self.percent.unwrap());
        self.status = "ok".into();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snapshot {
    pub schema_version: u32,
    pub supported: bool,
    pub platform: String,
    pub sampled_at: String,
    pub collection_ms: f64,
    pub resources: Vec<Resource>,
    pub issues: Vec<String>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            schema_version: 1,
            supported: cfg!(windows),
            platform: std::env::consts::OS.into(),
            sampled_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            collection_ms: 0.0,
            resources: Vec::new(),
            issues: if cfg!(windows) { Vec::new() } else { vec!["Monitor disponível apenas no Windows".into()] },
        }
    }
}

pub fn start(app: tauri::AppHandle) {
    #[cfg(windows)]
    {
        use std::time::{Duration, Instant};
        use tauri::{Emitter, Manager};
        std::thread::Builder::new().name("machine-monitor".into()).spawn(move || {
            let mut collector = None;
            let mut due = Instant::now();
            loop {
                let state = app.state::<crate::AppState>();
                let preferences = state.cfg.lock().unwrap().machine_monitor.clone();
                if !preferences.enabled {
                    collector = None;
                    due = Instant::now();
                } else if Instant::now() >= due {
                    let native = collector.get_or_insert_with(windows::NativeCollector::new);
                    let snapshot = native.sample();
                    *state.machine_monitor.lock().unwrap() = snapshot.clone();
                    let _ = app.emit("machine_snapshot", &snapshot);
                    due = Instant::now() + Duration::from_secs(u64::from(preferences.refresh_seconds.clamp(3, 10)));
                }
                std::thread::sleep(Duration::from_millis(250));
            }
        }).ok();
    }
    #[cfg(not(windows))]
    let _ = app;
}

/// A read-only diagnostic, without provider authentication or configuration writes.
pub fn smoke() -> Result<Snapshot, String> {
    #[cfg(windows)]
    {
        let mut collector = windows::NativeCollector::new();
        collector.sample();
        std::thread::sleep(std::time::Duration::from_secs(3));
        Ok(collector.sample())
    }
    #[cfg(not(windows))]
    Ok(Snapshot::default())
}

#[cfg(any(windows, test))]
fn valid_elapsed(seconds: f64) -> bool {
    seconds.is_finite() && seconds > 0.0 && seconds <= 30.0
}

#[cfg(any(windows, test))]
fn cpu_percent(previous: (u64, u64, u64), current: (u64, u64, u64), seconds: f64) -> Option<f64> {
    if !valid_elapsed(seconds) { return None; }
    let idle = current.0.checked_sub(previous.0)?;
    let total = current.1.checked_sub(previous.1)?.checked_add(current.2.checked_sub(previous.2)?)?;
    if total == 0 || idle > total { return None; }
    Some((100.0 * (1.0 - idle as f64 / total as f64)).clamp(0.0, 100.0))
}

#[cfg(any(windows, test))]
fn byte_rates(previous: (u64, u64), current: (u64, u64), seconds: f64) -> Option<(f64, f64)> {
    if !valid_elapsed(seconds) { return None; }
    Some((current.0.checked_sub(previous.0)? as f64 / seconds,
          current.1.checked_sub(previous.1)? as f64 / seconds))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_default_roundtrip_and_absent_ids_are_preserved() {
        let settings: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(settings, Settings::default());
        let chosen = Settings { order: vec!["gpu-unplugged".into(), "cpu".into(), "cpu".into()],
            hidden: vec!["cpu".into(), "memory".into(), "gpu-unplugged".into()], ..settings };
        let validated = chosen.validate().unwrap();
        assert_eq!(validated.order, ["gpu-unplugged", "cpu"]);
        let restored: Settings = serde_json::from_str(&serde_json::to_string(&validated).unwrap()).unwrap();
        assert_eq!(restored, validated);
        assert_eq!(restored.hidden, chosen.hidden);
    }
    #[test]
    fn settings_reject_invalid_intervals_and_unbounded_ids() {
        assert!(Settings { refresh_seconds: 0, ..Settings::default() }.validate().is_err());
        assert!(Settings { history_minutes: 2, ..Settings::default() }.validate().is_err());
        for id in [String::new(), "x".repeat(161), "cpu\n".into(), "cpu\0".into()] {
            assert!(Settings { order: vec![id], ..Settings::default() }.validate().is_err());
        }
        assert!(Settings { hidden: vec!["cpu".into(); 129], ..Settings::default() }.validate().is_err());
        assert!(serde_json::from_str::<Settings>(r#"{"enabled":"yes"}"#).is_err());
    }
    #[test]
    fn counter_resets_gaps_and_zero_work_are_distinct() {
        assert_eq!(byte_rates((10, 20), (10, 20), 3.0), Some((0.0, 0.0)));
        assert_eq!(byte_rates((10, 20), (40, 80), 3.0), Some((10.0, 20.0)));
        for elapsed in [0.0, -1.0, 30.1, f64::NAN, f64::INFINITY] {
            assert_eq!(byte_rates((0, 0), (3, 4), elapsed), None);
            assert_eq!(cpu_percent((0, 0, 0), (1, 2, 2), elapsed), None);
        }
        assert_eq!(byte_rates((10, 20), (9, 21), 3.0), None);
        assert_eq!(cpu_percent((0, 0, 0), (80, 100, 100), 3.0), Some(60.0));
        assert_eq!(cpu_percent((10, 20, 20), (9, 40, 40), 3.0), None);
        assert_eq!(cpu_percent((0, 0, 0), (10, 1, 1), 3.0), None);
        assert_eq!(cpu_percent((0, 0, 0), (0, 0, 0), 3.0), None);
    }
    #[test]
    fn snapshot_contains_no_temperature_guess_or_provider_fields() {
        let value = serde_json::to_value(Snapshot::default()).unwrap();
        assert_eq!(value["schema_version"], 1);
        assert!(value.get("credentials").is_none());
        assert!(value.get("provider").is_none());
        #[cfg(not(windows))]
        assert_eq!(value["supported"], false);
    }
    #[cfg(windows)]
    #[test]
    #[ignore = "read-only live hardware diagnostic; takes three seconds"]
    fn live_native_two_samples() {
        let snapshot = smoke().unwrap();
        assert!(snapshot.supported);
        for kind in ["cpu", "memory", "disk", "gpu", "network"] {
            assert!(snapshot.resources.iter().any(|resource| resource.kind == kind));
        }
        assert!(snapshot.resources.iter().all(|resource| resource.temperature_c.is_none()));
        assert!(snapshot.resources.iter().any(|resource| resource.kind == "cpu" && resource.percent.is_some()));
        assert!(snapshot.resources.iter().any(|resource| resource.kind == "memory" && resource.percent.is_some()));
        println!("{}", serde_json::to_string(&snapshot).unwrap());
    }
}
