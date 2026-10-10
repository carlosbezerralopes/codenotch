//! Location calendar for derived quota/time comparisons. No usage or authentication is read here.
use chrono::{Datelike, Duration as CalendarDuration, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use std::{fs, io::{Read, Write}, path::Path, sync::{Mutex, atomic::{AtomicU64, Ordering}}, time::Duration};
use tauri::{AppHandle, Emitter};

static SETTINGS_LOCK: Mutex<()> = Mutex::new(());
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);
const IP_SERVICE: &str = "https://ipapi.co/json/";
const MAX_IP_RESPONSE: u64 = 16 * 1024;
const NATIONAL_SOURCE: &str = "https://agenciagov.ebc.com.br/noticias/202512/confira-o-calendario-oficial-de-feriados-nacionais-e-pontos-facultativos-em-2026";
const CE_SOURCE: &str = "https://portaldoservidor.al.ce.gov.br/noticias/portaria-suspende-expediente-da-alece-na-data-magna-do-ceara-2503";
const FORTALEZA_SOURCE: &str = "https://sapl.fortaleza.ce.leg.br/media/sapl/public/normajuridica/2003/3331/3331_texto_integral.pdf";
const EXCEPTION_2026_SOURCE: &str = "https://sapl.fortaleza.ce.leg.br/media/sapl/public/normajuridica/2026/14390/lei_no_11.649_de_08_de_abril_de_2026_-_rfpl_0185-2026.pdf";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CalendarOverride {
    pub date: String,
    pub name: String,
    pub nonworking: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Holiday {
    pub date: String,
    pub name: String,
    pub source: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SourceCoverage {
    pub national: bool,
    pub state: bool,
    pub municipal: bool,
    pub verified_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CalendarSettings {
    pub country: String,
    pub state: String,
    pub city: String,
    pub utc_offset_minutes: i32,
    pub include_optional_days: bool,
    pub overrides: Vec<CalendarOverride>,
    // Computed fields are never trusted from an IPC request or a hand-edited configuration.
    #[serde(skip_deserializing)]
    pub holidays: Vec<Holiday>,
    #[serde(skip_deserializing)]
    pub covered_years: Vec<i32>,
    #[serde(skip_deserializing)]
    pub source_coverage: SourceCoverage,
    #[serde(skip_deserializing)]
    pub sources: Vec<String>,
}

impl Default for CalendarSettings {
    fn default() -> Self {
        Self { country: "BR".into(), state: "CE".into(), city: "Fortaleza".into(),
            utc_offset_minutes: -180, include_optional_days: false, overrides: Vec::new(),
            holidays: Vec::new(), covered_years: Vec::new(), source_coverage: SourceCoverage::default(), sources: Vec::new() }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DetectedLocation {
    pub country: String,
    pub state: String,
    pub city: String,
    pub timezone: String,
    pub utc_offset_minutes: i32,
    pub service: String,
}

fn clean_text(value: &str, maximum: usize) -> Result<String, String> {
    if value.chars().any(char::is_control) { return Err("Control characters are not allowed".into()); }
    let value = value.trim();
    if value.is_empty() || value.chars().count() > maximum {
        return Err("Invalid calendar location or date name".into());
    }
    Ok(value.to_string())
}

fn validated(mut settings: CalendarSettings, year: i32) -> Result<CalendarSettings, String> {
    settings.country = clean_text(&settings.country, 2)?.to_uppercase();
    settings.state = clean_text(&settings.state, 16)?.to_uppercase();
    settings.city = clean_text(&settings.city, 120)?;
    if settings.country.len() != 2 || !settings.country.bytes().all(|c| c.is_ascii_uppercase())
        || !settings.state.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-') {
        return Err("Use a two-letter country and a valid state or region code".into());
    }
    const BRAZIL_STATES: [&str; 27] = ["AC","AL","AP","AM","BA","CE","DF","ES","GO","MA","MT","MS","MG","PA","PB","PR","PE","PI","RJ","RN","RS","RO","RR","SC","SP","SE","TO"];
    if settings.country == "BR" && !BRAZIL_STATES.contains(&settings.state.as_str()) {
        return Err("Invalid Brazilian state code".into());
    }
    if !(-720..=840).contains(&settings.utc_offset_minutes) {
        return Err("UTC offset must be between -720 and 840 minutes".into());
    }
    if settings.include_optional_days {
        return Err("Add optional days explicitly as custom nonworking dates".into());
    }
    if settings.overrides.len() > 512 { return Err("At most 512 custom dates are supported".into()); }
    let mut seen = std::collections::HashSet::new();
    for entry in &mut settings.overrides {
        if entry.date.len() != 10 || NaiveDate::parse_from_str(&entry.date, "%Y-%m-%d").is_err()
            || !seen.insert(entry.date.clone()) {
            return Err("Custom dates must be distinct YYYY-MM-DD dates".into());
        }
        entry.name = clean_text(&entry.name, 80)?;
    }
    settings.overrides.sort_by(|a,b| a.date.cmp(&b.date));
    let national = settings.country == "BR";
    let ce = national && settings.state == "CE";
    let fortaleza = ce && settings.city.eq_ignore_ascii_case("Fortaleza");
    settings.source_coverage = SourceCoverage { national, state: ce, municipal: fortaleza, verified_at: "2026-10-10".into() };
    settings.sources = if national { vec![NATIONAL_SOURCE.into()] } else { Vec::new() };
    if ce { settings.sources.push(CE_SOURCE.into()); }
    if fortaleza { settings.sources.extend([FORTALEZA_SOURCE.into(), EXCEPTION_2026_SOURCE.into()]); }
    settings.covered_years = vec![year - 1, year, year + 1];
    settings.holidays = settings.covered_years.iter().flat_map(|year| holidays_for(*year, national, ce, fortaleza)).collect();
    settings.holidays.sort_by(|a,b| a.date.cmp(&b.date));
    Ok(settings)
}

// Gregorian Easter, used only for the religious dates declared in the source calendar.
fn easter(year: i32) -> NaiveDate {
    let a=year%19; let b=year/100; let c=year%100; let d=b/4; let e=b%4;
    let f=(b+8)/25; let g=(b-f+1)/3; let h=(19*a+b-d-g+15)%30;
    let i=c/4; let k=c%4; let l=(32+2*e+2*i-h-k)%7; let m=(a+11*h+22*l)/451;
    let month=(h+l-7*m+114)/31; let day=(h+l-7*m+114)%31+1;
    NaiveDate::from_ymd_opt(year, month as u32, day as u32).expect("Gregorian Easter date")
}

fn holidays_for(year: i32, national: bool, ce: bool, fortaleza: bool) -> Vec<Holiday> {
    let mut dates = Vec::new();
    let mut add = |date: NaiveDate, name: &str, source: &str| dates.push(Holiday { date: date.to_string(), name: name.into(), source: source.into() });
    if national {
        for (month, day, name) in [(1,1,"Confraternização Universal"),(4,21,"Tiradentes"),(5,1,"Dia do Trabalho"),
            (9,7,"Independência do Brasil"),(10,12,"Nossa Senhora Aparecida"),(11,2,"Finados"),
            (11,15,"Proclamação da República"),(12,25,"Natal")] {
            add(NaiveDate::from_ymd_opt(year,month,day).unwrap(),name,NATIONAL_SOURCE);
        }
        if year >= 2024 { add(NaiveDate::from_ymd_opt(year,11,20).unwrap(),"Consciência Negra",NATIONAL_SOURCE); }
        add(easter(year)-CalendarDuration::days(2),"Sexta-feira da Paixão",NATIONAL_SOURCE);
    }
    if ce { add(NaiveDate::from_ymd_opt(year,3,25).unwrap(),"Data Magna do Ceará",CE_SOURCE); }
    if fortaleza {
        add(NaiveDate::from_ymd_opt(year,3,19).unwrap(),"São José",FORTALEZA_SOURCE);
        add(NaiveDate::from_ymd_opt(year,8,15).unwrap(),"Nossa Senhora da Assunção",FORTALEZA_SOURCE);
        add(easter(year)+CalendarDuration::days(60),"Corpus Christi",FORTALEZA_SOURCE);
        // Lei 11.649/2026 expressly applies only to this year's tricentenary.
        if year == 2026 { add(NaiveDate::from_ymd_opt(year,4,13).unwrap(),"Tricentenário de Fortaleza",EXCEPTION_2026_SOURCE); }
    }
    dates
}

fn read_settings() -> Result<CalendarSettings, String> {
    let path = crate::config::config_path().with_file_name("work-calendar.json");
    let settings = match fs::File::open(path) {
        Ok(file) => {
            let mut bytes = Vec::new();
            file.take(256 * 1024 + 1).read_to_end(&mut bytes).map_err(|error| error.to_string())?;
            if bytes.len() > 256 * 1024 { return Err("Calendar configuration is too large".into()); }
            serde_json::from_slice(&bytes).map_err(|error| format!("Invalid calendar configuration: {error}"))?
        },
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => CalendarSettings::default(),
        Err(error) => return Err(error.to_string()),
    };
    validated(settings, Utc::now().year())
}

fn save_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path.parent().ok_or("Calendar path has no parent")?;
    fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    let temp = parent.join(format!(".work-calendar-{}-{}.tmp", std::process::id(), TEMP_COUNTER.fetch_add(1, Ordering::Relaxed)));
    let result = (|| {
        let mut file = fs::OpenOptions::new().write(true).create_new(true).open(&temp).map_err(|error| error.to_string())?;
        file.write_all(bytes).and_then(|_| file.sync_all()).map_err(|error| error.to_string())?;
        drop(file);
        #[cfg(windows)] {
            use std::os::windows::ffi::OsStrExt;
            #[link(name="kernel32")] extern "system" { fn MoveFileExW(from:*const u16, to:*const u16, flags:u32) -> i32; }
            let from: Vec<u16> = temp.as_os_str().encode_wide().chain(Some(0)).collect();
            let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
            if unsafe { MoveFileExW(from.as_ptr(),to.as_ptr(),0x1|0x8) } == 0 { return Err(std::io::Error::last_os_error().to_string()); }
        }
        #[cfg(not(windows))] fs::rename(&temp,path).map_err(|error| error.to_string())?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&temp); }
    result
}

#[tauri::command]
pub fn get_calendar_settings() -> Result<CalendarSettings, String> {
    let _guard = SETTINGS_LOCK.lock().map_err(|_| "Calendar configuration lock failed")?;
    read_settings()
}

#[tauri::command]
pub fn set_calendar_settings(app: AppHandle, settings: CalendarSettings) -> Result<CalendarSettings, String> {
    let _guard = SETTINGS_LOCK.lock().map_err(|_| "Calendar configuration lock failed")?;
    let settings = validated(settings, Utc::now().year())?;
    let path = crate::config::config_path().with_file_name("work-calendar.json");
    // Persist inputs only; derived holiday/coverage fields are regenerated when read.
    let mut stored = serde_json::to_value(&settings).map_err(|error| error.to_string())?;
    if let Some(object) = stored.as_object_mut() { for key in ["holidays","covered_years","source_coverage","sources"] { object.remove(key); } }
    save_atomic(&path,&serde_json::to_vec_pretty(&stored).map_err(|error| error.to_string())?)?;
    let _ = app.emit("work_calendar_changed", &settings);
    Ok(settings)
}

fn parse_utc_offset(offset: &str) -> Result<i32, String> {
    if offset.len()!=5 || !matches!(offset.as_bytes()[0],b'+'|b'-') || !offset.as_bytes()[1..].iter().all(u8::is_ascii_digit) {
        return Err("IP location did not report a valid UTC offset".into());
    }
    let hours: i32 = offset[1..3].parse().map_err(|_| "Invalid UTC offset")?;
    let minutes: i32 = offset[3..5].parse().map_err(|_| "Invalid UTC offset")?;
    if minutes>=60 { return Err("Invalid UTC offset".into()); }
    let offset=(hours*60+minutes)*if offset.starts_with('-') {-1} else {1};
    if !(-720..=840).contains(&offset) { return Err("Invalid UTC offset".into()); }
    Ok(offset)
}

fn detected_location(bytes: &[u8]) -> Result<DetectedLocation, String> {
    if bytes.len()>MAX_IP_RESPONSE as usize { return Err("IP location response is too large".into()); }
    let value: serde_json::Value = serde_json::from_slice(bytes).map_err(|_| "Invalid IP location response")?;
    if value.get("error").and_then(|value| value.as_bool()) == Some(true) { return Err("IP location service could not locate this connection".into()); }
    let text = |key| value.get(key).and_then(|value| value.as_str()).ok_or_else(|| format!("IP location is missing {key}"));
    let settings = validated(CalendarSettings { country: text("country")?.into(), state: text("region_code")?.into(), city: text("city")?.into(),
        utc_offset_minutes: parse_utc_offset(text("utc_offset")?)?, ..Default::default() }, Utc::now().year())?;
    Ok(DetectedLocation { country: settings.country, state: settings.state, city: settings.city,
        timezone: clean_text(text("timezone")?,100)?, utc_offset_minutes: settings.utc_offset_minutes, service: "ipapi.co".into() })
}

#[tauri::command]
pub async fn detect_calendar_location() -> Result<DetectedLocation, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let agent = ureq::AgentBuilder::new().timeout(Duration::from_secs(5)).redirects(0).build();
        let response = agent.get(IP_SERVICE).call().map_err(|_| "IP location lookup failed or timed out".to_string())?;
        let mut bytes = Vec::new();
        response.into_reader().take(MAX_IP_RESPONSE+1).read_to_end(&mut bytes).map_err(|_| "IP location response could not be read")?;
        detected_location(&bytes)
    }).await.map_err(|_| "IP location task failed".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fortaleza_calendar_has_religious_dates_and_one_year_exception_only() {
        let settings=validated(CalendarSettings::default(),2026).unwrap();
        for date in ["2026-03-19","2026-03-25","2026-04-03","2026-04-13","2026-06-04","2026-08-15","2026-10-12"] {
            assert!(settings.holidays.iter().any(|holiday| holiday.date==date));
        }
        assert!(!settings.holidays.iter().any(|holiday| holiday.date=="2027-04-13"));
        assert!(!settings.holidays.iter().any(|holiday| holiday.date=="2026-02-16"));
        assert!(!settings.holidays.iter().any(|holiday| holiday.date=="2026-04-02"));
        assert!(settings.source_coverage.municipal);
        let other=validated(CalendarSettings { city:"Sobral".into(), ..Default::default() },2026).unwrap();
        assert!(!other.source_coverage.municipal);
        assert!(!other.holidays.iter().any(|holiday| holiday.date=="2026-03-19"));
    }
    #[test]
    fn invalid_locations_overrides_and_ip_replies_are_refused() {
        assert!(validated(CalendarSettings { state:"XX".into(), ..Default::default() },2026).is_err());
        assert!(validated(CalendarSettings { city:"".into(), ..Default::default() },2026).is_err());
        assert!(validated(CalendarSettings { overrides:vec![CalendarOverride{date:"2026-02-30".into(),name:"Custom".into(),nonworking:true}], ..Default::default() },2026).is_err());
        assert!(detected_location(br#"{"error":true}"#).is_err());
        assert!(detected_location(br#"{"country":"BR","region_code":"CE","city":"Fortaleza","timezone":"America/Fortaleza","utc_offset":"-0360"}"#).is_err());
        let location=detected_location(br#"{"country":"BR","region_code":"CE","city":"Fortaleza","timezone":"America/Fortaleza","utc_offset":"-0300","ip":"unused"}"#).unwrap();
        assert_eq!(location.utc_offset_minutes,-180);
        assert!(detected_location(&vec![b' ';MAX_IP_RESPONSE as usize+1]).is_err());
    }
}
