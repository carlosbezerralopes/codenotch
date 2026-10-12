//! Shared quota references for the notch and tray. Inputs are explicit; no IO, clock,
//! authentication, telemetry extrapolation, or exhaustion forecast belongs here.
use crate::usage::{claude_duration_seconds, CreditScope, LimitWindow, UsageSnapshot};
use crate::work_calendar::CalendarSettings;
use chrono::{Datelike, Duration, FixedOffset, NaiveDate, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

const DAY_MS: f64 = 86_400_000.0;
const MAX_PERIOD_SECONDS: u64 = 366 * 86400;
const FRESH_FOR_MS: u64 = 15 * 60 * 1000;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInsights {
    pub provider: String,
    pub generated_at: u64,
    pub windows: Vec<WindowInsights>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowInsights {
    pub window_index: usize,
    pub window_id: String,
    pub label: String,
    pub group: Option<String>,
    pub fresh: bool,
    pub official_used_percent: Option<f64>,
    pub official_remaining_percent: Option<f64>,
    pub credits: Option<CreditBalance>,
    pub period: Option<PeriodInsights>,
    pub work: Option<WorkInsights>,
    pub budget: Option<BudgetInsights>,
    pub reasons: Vec<InsightReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quota_role: Option<QuotaRole>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub period_kind: Option<PeriodKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projection: Option<QuotaProjection>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_budget: Option<DisplayBudget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_reading_reference: Option<LastReadingReference>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QuotaRole { Session, General, Scoped, Other }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodKind { Session, Weekly, Monthly, Unknown }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionState {
    BeforeReset, CoversReset, Exhausted, ProjectedDue, InsufficientHistory,
    IdentityUnknown, InsufficientWorkTime, InsufficientCoverage, NoObservedIncrease,
    NoRemainingWorkTime, HistoryIncomplete, HistoryTruncated, Unavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionBasis { ObservedRecent, CycleMean }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProjectionReason {
    Stale, MissingOrigin, InvalidReading, MissingReset, ResetExpired, InvalidPeriod,
    UnknownStart, UnsupportedWindow, UnsupportedCreditScope, CalendarMissing,
    CalendarOutsideCoverage, CalendarPartial, InsufficientWorkTime,
    InsufficientHistory, IdentityUnknown, RelativeReset, InsufficientCoverage,
    NoObservedIncrease, NoRemainingWorkTime, HistoryWarming, HistoryIncomplete,
    HistoryTruncated, HistoryUnavailable, HistoryConflict, HistoryRevision,
    MixedOrExtraConsumption, CycleMeanIncludesExtras, NoRecentRateMeasured,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaProjection {
    pub state: ProjectionState,
    pub basis: Option<ProjectionBasis>,
    pub unit: BudgetUnit,
    pub sampled_from_ms: Option<u64>,
    pub sampled_to_ms: Option<u64>,
    pub sample_count: usize,
    pub observed_workdays: Option<f64>,
    pub coverage_fraction: Option<f64>,
    pub rate_per_workday: Option<f64>,
    #[serde(default)]
    pub rate_percent: Option<f64>,
    pub exhaustion_at_ms: Option<u64>,
    pub exhaustion_workdays: Option<f64>,
    pub remaining_workdays: Option<f64>,
    pub early_workdays: Option<f64>,
    pub early_calendar_ms: Option<u64>,
    pub margin_at_reset: Option<f64>,
    pub extra_or_mixed_consumption: bool,
    pub reasons: Vec<ProjectionReason>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayBudgetMode { SessionQuotaReset, PerWorkday, UntilReset, ExtraDaysUntilReset, Unavailable }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayBudget {
    pub mode: DisplayBudgetMode,
    pub unit: BudgetUnit,
    pub amount: Option<f64>,
    #[serde(default)]
    pub amount_percent: Option<f64>,
    pub horizon_start_ms: Option<u64>,
    pub horizon_end_ms: Option<u64>,
    pub workdays_remaining: Option<f64>,
    pub as_of_ms: Option<u64>,
    pub is_current: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastReadingReference {
    pub as_of_ms: u64,
    pub age_ms: u64,
    pub official_remaining: Option<f64>,
    pub unit: BudgetUnit,
    pub per_remaining_workday_as_of: Option<f64>,
    pub reset_ms: Option<u64>,
    pub display_budget: Option<DisplayBudget>,
    pub previous_cycle: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CreditBalance {
    pub consumed: f64,
    pub allowance: f64,
    pub remaining: f64,
    pub scope: CreditScope,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodInsights {
    pub start_ms: Option<u64>,
    pub reset_ms: u64,
    pub duration_seconds: Option<u64>,
    pub basis: PeriodBasis,
    pub elapsed_percent: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkInsights {
    pub cycle_total_days: Option<f64>,
    pub cycle_elapsed_days: Option<f64>,
    pub remaining_days: f64,
    pub elapsed_percent: Option<f64>,
    pub used_minus_work_pp: Option<f64>,
    pub calendar_partial: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BudgetInsights {
    pub unit: BudgetUnit,
    pub remaining: f64,
    pub per_remaining_workday: f64,
    pub reference_per_cycle_workday: Option<f64>,
    pub week_start_ms: u64,
    pub week_end_ms: u64,
    pub week_remaining_workdays: f64,
    pub week_remaining_budget: f64,
    pub week_reference_budget: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BudgetUnit { Percent, Credits }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PeriodBasis {
    ProviderDuration,
    OfficialWeekly,
    OfficialSession,
    OfficialCalendarMonth,
    ResetOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InsightReason {
    Stale,
    MissingReading,
    MissingWindowFreshness,
    InvalidReading,
    MissingReset,
    ResetExpired,
    InvalidPeriod,
    UnknownStart,
    CalendarMissing,
    CalendarOutsideCoverage,
    NoWorkdays,
    NoRemainingWorkdays,
    CalendarPartial,
    UnknownCreditScope,
    NonrenewingWallet,
}

/// JSON preserves nulls for unknown values, and source ordering for indicator links.
pub fn compute_provider_insights(
    provider: &str,
    snapshot: &UsageSnapshot,
    calendar: Option<&CalendarSettings>,
    now_ms: u64,
) -> ProviderInsights {
    ProviderInsights {
        provider: provider.into(), generated_at: now_ms,
        windows: snapshot.windows.iter().enumerate()
            .map(|(index, window)| {
                let origin = window_origin(provider, snapshot, window);
                let fresh = snapshot.status == "ok" && origin.is_some_and(|(at, status)| status == "ok" && at > 0
                    && at <= now_ms && now_ms - at <= FRESH_FOR_MS);
                let reason = if snapshot.status != "ok" { InsightReason::Stale }
                    else if origin.is_none() { InsightReason::MissingWindowFreshness }
                    else if origin.is_some_and(|(at, _)| at == 0) { InsightReason::MissingReading }
                    else { InsightReason::Stale };
                compute_window(provider, window, index, fresh, reason, calendar, now_ms)
            })
            .collect(),
    }
}

pub(crate) fn window_origin<'a>(provider: &str, snapshot: &'a UsageSnapshot, window: &'a LimitWindow) -> Option<(u64, &'a str)> {
    match (window.reading_fetched_at, window.reading_status.as_deref()) {
        (Some(at), Some(status)) => Some((at, status)),
        // Old single-account Claude cards were ungrouped and used unsuffixed IDs.
        // A group or account suffix makes the aggregate's timestamp ambiguous.
        (None, None) if provider != "claude" || snapshot.windows.iter().all(|w| w.group.is_none() && !w.id.contains('@')) => {
            Some((snapshot.fetched_at, &snapshot.status))
        }
        _ => None,
    }
}

fn compute_window(
    provider: &str, w: &LimitWindow, index: usize, fresh: bool, freshness_reason: InsightReason,
    calendar: Option<&CalendarSettings>, now: u64,
) -> WindowInsights {
    let valid_used = w.used.is_finite() && (0.0..=1.0).contains(&w.used)
        && w.count.is_none() && !w.derived;
    let used = valid_used.then_some(w.used * 100.0);
    let credits = w.credits.as_ref().filter(|c| c.consumed.is_finite() && c.consumed >= 0.0
        && c.allowance.is_finite() && c.allowance > 0.0).map(|c| CreditBalance {
            consumed: c.consumed, allowance: c.allowance,
            remaining: (c.allowance - c.consumed).max(0.0),
            scope: w.credit_scope.unwrap_or(CreditScope::Unknown),
        });
    let mut out = WindowInsights {
        window_index: index, window_id: w.id.clone(), label: w.label.clone(), group: w.group.clone(),
        fresh, official_used_percent: used, official_remaining_percent: used.map(|p| 100.0 - p),
        credits, period: None, work: None, budget: None, reasons: Vec::new(),
        quota_role: None, period_kind: None, projection: None, display_budget: None,
        last_reading_reference: None,
    };
    if !matches!(provider, "claude" | "codex" | "kiro") { return out; }
    if !fresh {
        out.reasons.push(freshness_reason);
        return out;
    }
    if !valid_used {
        out.reasons.push(InsightReason::InvalidReading);
        return out;
    }
    // Bonus expiry and metered overage are not renewing monthly plan allowances.
    if provider == "kiro" && w.id != "credits" {
        out.reasons.push(InsightReason::NonrenewingWallet);
        return out;
    }
    let Some(reset) = w.resets_at else {
        out.reasons.push(InsightReason::MissingReset);
        return out;
    };
    if reset <= now {
        out.reasons.push(InsightReason::ResetExpired);
        return out;
    }
    let (duration, basis) = period_duration(provider, w, reset);
    let start = match duration {
        Some(d) => {
            let start = d.checked_mul(1000).and_then(|ms| reset.checked_sub(ms));
            match start.filter(|start| *start > 0 && *start <= now) {
                Some(start) => Some(start),
                None => { out.reasons.push(InsightReason::InvalidPeriod); return out; }
            }
        }
        None => { out.reasons.push(InsightReason::UnknownStart); None }
    };
    out.period = Some(PeriodInsights {
        start_ms: start, reset_ms: reset, duration_seconds: duration, basis,
        elapsed_percent: start.map(|s| (now - s) as f64 / (reset - s) as f64 * 100.0),
    });
    if provider == "kiro" && w.credit_scope != Some(CreditScope::Plan) {
        out.reasons.push(InsightReason::UnknownCreditScope);
        return out;
    }
    let Some(calendar) = calendar else {
        out.reasons.push(InsightReason::CalendarMissing);
        return out;
    };
    let work = match WorkingCalendar::new(calendar) {
        Ok(work) => work,
        Err(reason) => { out.reasons.push(reason); return out; }
    };
    let (remaining, total, elapsed) = match (|| {
        let remaining = work.days(now, reset)?;
        let (total, elapsed) = if let Some(start) = start {
            (Some(work.days(start, reset)?), Some(work.days(start, now)?))
        } else { (None, None) };
        Ok::<_, InsightReason>((remaining, total, elapsed))
    })() {
        Ok(values) => values,
        Err(reason) => { out.reasons.push(reason); return out; }
    };
    if total == Some(0.0) {
        out.reasons.push(InsightReason::NoWorkdays);
        return out;
    }
    let partial = !(calendar.source_coverage.national && calendar.source_coverage.state
        && calendar.source_coverage.municipal);
    if partial { out.reasons.push(InsightReason::CalendarPartial); }
    let work_percent = elapsed.zip(total).map(|(e, t)| e / t * 100.0);
    out.work = Some(WorkInsights {
        cycle_total_days: total, cycle_elapsed_days: elapsed, remaining_days: remaining,
        elapsed_percent: work_percent, used_minus_work_pp: work_percent.map(|p| w.used * 100.0 - p),
        calendar_partial: partial,
    });
    if remaining <= 0.0 {
        out.reasons.push(InsightReason::NoRemainingWorkdays);
        return out;
    }
    let (unit, balance, allowance) = if provider == "kiro" {
        let Some(c) = out.credits.as_ref() else {
            out.reasons.push(InsightReason::MissingReading);
            return out;
        };
        (BudgetUnit::Credits, c.remaining, c.allowance)
    } else { (BudgetUnit::Percent, (1.0 - w.used) * 100.0, 100.0) };
    let (week_start, week_end) = match work.week_bounds(now) {
        Ok(bounds) => bounds,
        Err(reason) => { out.reasons.push(reason); return out; }
    };
    let (week_remaining, week_reference) = match (|| {
        let week_remaining = work.days(now, reset.min(week_end))?;
        let week_reference = start.zip(total).map(|(s, t)| {
            work.days(s.max(week_start), reset.min(week_end)).map(|days| allowance * days / t)
        }).transpose()?;
        Ok::<_, InsightReason>((week_remaining, week_reference))
    })() {
        Ok(values) => values,
        Err(reason) => { out.reasons.push(reason); return out; }
    };
    out.budget = Some(BudgetInsights {
        unit, remaining: balance, per_remaining_workday: balance / remaining,
        reference_per_cycle_workday: total.map(|t| allowance / t),
        week_start_ms: week_start, week_end_ms: week_end,
        week_remaining_workdays: week_remaining,
        week_remaining_budget: balance * (week_remaining / remaining).clamp(0.0, 1.0),
        week_reference_budget: week_reference,
    });
    out
}

fn period_duration(provider: &str, w: &LimitWindow, reset: u64) -> (Option<u64>, PeriodBasis) {
    if provider == "kiro" {
        return if w.credit_scope == Some(CreditScope::Plan) {
            match crate::kiro::monthly_duration_seconds(reset) {
                Some(duration) => (Some(duration), PeriodBasis::OfficialCalendarMonth),
                None => (None, PeriodBasis::ResetOnly),
            }
        } else { (None, PeriodBasis::ResetOnly) };
    }
    if provider == "claude" {
        if let Some(duration) = claude_duration_seconds(w.id.split('@').next().unwrap_or(&w.id)) {
            return (Some(duration), if duration == 18000 {
                PeriodBasis::OfficialSession
            } else { PeriodBasis::OfficialWeekly });
        }
    }
    match w.duration_seconds.filter(|duration| *duration > 0 && *duration <= MAX_PERIOD_SECONDS) {
        Some(duration) => (Some(duration), PeriodBasis::ProviderDuration),
        None => (None, PeriodBasis::ResetOnly),
    }
}

pub(crate) struct WorkingCalendar<'a> {
    settings: &'a CalendarSettings,
    offset: FixedOffset,
    holidays: HashSet<NaiveDate>,
    overrides: HashMap<NaiveDate, bool>,
}

impl<'a> WorkingCalendar<'a> {
    pub(crate) fn new(settings: &'a CalendarSettings) -> Result<Self, InsightReason> {
        if !(-720..=840).contains(&settings.utc_offset_minutes) { return Err(InsightReason::CalendarMissing); }
        let offset = FixedOffset::east_opt(settings.utc_offset_minutes * 60).ok_or(InsightReason::CalendarMissing)?;
        let date = |raw: &str| if raw.len() == 10 {
            NaiveDate::parse_from_str(raw, "%Y-%m-%d").map_err(|_| InsightReason::CalendarMissing)
        } else { Err(InsightReason::CalendarMissing) };
        let holidays = settings.holidays.iter().map(|h| date(&h.date)).collect::<Result<HashSet<_>, _>>()?;
        let mut overrides = HashMap::new();
        for entry in &settings.overrides {
            if overrides.insert(date(&entry.date)?, entry.nonworking).is_some() { return Err(InsightReason::CalendarMissing); }
        }
        Ok(Self { settings, offset, holidays, overrides })
    }

    fn local_date(&self, ms: u64) -> Result<NaiveDate, InsightReason> {
        chrono::DateTime::<Utc>::from_timestamp_millis(i64::try_from(ms).map_err(|_| InsightReason::InvalidPeriod)?)
            .map(|d| d.with_timezone(&self.offset).date_naive()).ok_or(InsightReason::InvalidPeriod)
    }

    fn midnight(&self, date: NaiveDate) -> Result<i64, InsightReason> {
        self.offset.from_local_datetime(&date.and_hms_opt(0, 0, 0).ok_or(InsightReason::InvalidPeriod)?)
            .single().map(|d| d.timestamp_millis()).ok_or(InsightReason::InvalidPeriod)
    }

    /// Equivalent 24-hour working dates. The first, current and last date can be fractional.
    pub(crate) fn days(&self, from: u64, to: u64) -> Result<f64, InsightReason> {
        Ok(self.useful_segments(from, to)?.iter().map(|(l, r)| r - l).sum::<u64>() as f64 / DAY_MS)
    }

    /// Exact integer segments also prove whether an entire adjacent sample pair is useful.
    pub(crate) fn useful_segments(&self, from: u64, to: u64) -> Result<Vec<(u64, u64)>, InsightReason> {
        if to <= from { return Ok(Vec::new()); }
        if to - from > MAX_PERIOD_SECONDS * 1000 { return Err(InsightReason::InvalidPeriod); }
        let from_ms = i64::try_from(from).map_err(|_| InsightReason::InvalidPeriod)?;
        let to_ms = i64::try_from(to).map_err(|_| InsightReason::InvalidPeriod)?;
        let mut date = self.local_date(from)?;
        let mut segments = Vec::new();
        loop {
            let day_start = self.midnight(date)?;
            if day_start >= to_ms { break; }
            if !self.settings.covered_years.contains(&date.year()) { return Err(InsightReason::CalendarOutsideCoverage); }
            let next = date.checked_add_signed(Duration::days(1)).ok_or(InsightReason::InvalidPeriod)?;
            let day_end = self.midnight(next)?;
            let nonworking = self.overrides.get(&date).copied().unwrap_or_else(|| self.holidays.contains(&date));
            if date.weekday().num_days_from_monday() < 5 && !nonworking {
                let left = from_ms.max(day_start);
                let right = to_ms.min(day_end);
                if right > left { segments.push((left as u64, right as u64)); }
            }
            date = next;
        }
        Ok(segments)
    }

    pub(crate) fn timestamp_after_workdays(&self, from: u64, limit: u64, days: f64) -> Result<Option<u64>, InsightReason> {
        if !days.is_finite() || days < 0.0 { return Err(InsightReason::InvalidPeriod); }
        if days == 0.0 { return Ok(Some(from.min(limit))); }
        let mut needed = days * DAY_MS;
        for (left, right) in self.useful_segments(from, limit)? {
            let length = (right - left) as f64;
            if needed <= length { return Ok(Some(left.saturating_add(needed.ceil() as u64).min(right).min(limit))); }
            needed -= length;
        }
        Ok(None)
    }

    pub(crate) fn timestamp_before_workdays(&self, from: u64, cycle_start: u64, days: f64) -> Result<u64, InsightReason> {
        if !days.is_finite() || days < 0.0 { return Err(InsightReason::InvalidPeriod); }
        if days == 0.0 { return Ok(from); }
        let mut needed = days * DAY_MS;
        for (left, right) in self.useful_segments(cycle_start, from)?.into_iter().rev() {
            let length = (right - left) as f64;
            if needed <= length { return Ok(right.saturating_sub(needed.ceil() as u64).max(left).max(cycle_start)); }
            needed -= length;
        }
        Ok(cycle_start)
    }

    fn week_bounds(&self, now: u64) -> Result<(u64, u64), InsightReason> {
        let date = self.local_date(now)?;
        let monday = date.checked_sub_signed(Duration::days(date.weekday().num_days_from_monday() as i64))
            .ok_or(InsightReason::InvalidPeriod)?;
        let next = monday.checked_add_signed(Duration::days(7)).ok_or(InsightReason::InvalidPeriod)?;
        Ok((u64::try_from(self.midnight(monday)?).map_err(|_| InsightReason::InvalidPeriod)?,
            u64::try_from(self.midnight(next)?).map_err(|_| InsightReason::InvalidPeriod)?))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::usage::CreditAmounts;
    use crate::work_calendar::{CalendarOverride, Holiday, SourceCoverage};

    fn epoch(raw: &str) -> u64 {
        chrono::DateTime::parse_from_rfc3339(raw).unwrap().timestamp_millis() as u64
    }
    fn calendar() -> CalendarSettings {
        CalendarSettings { covered_years: vec![2025, 2026, 2027, 2028],
            holidays: vec![Holiday { date: "2026-10-12".into(), name: "Holiday".into(), source: "fixture".into() }],
            source_coverage: SourceCoverage { national: true, state: true, municipal: true, ..Default::default() },
            ..Default::default() }
    }
    fn window(id: &str, used: f64, reset: &str, duration: Option<u64>) -> LimitWindow {
        LimitWindow { id: id.into(), used, resets_at: Some(epoch(reset)), duration_seconds: duration, ..Default::default() }
    }
    fn insight(provider: &str, w: LimitWindow, now: &str, calendar: &CalendarSettings) -> WindowInsights {
        let now = epoch(now);
        let snap = UsageSnapshot { status: "ok".into(), fetched_at: now, windows: vec![w], ..Default::default() };
        compute_provider_insights(provider, &snap, Some(calendar), now).windows.remove(0)
    }
    fn near(left: f64, right: f64) { assert!((left - right).abs() < 1e-8, "{left} != {right}"); }

    #[test]
    fn cycle_reset_tuesday_clips_week_and_integrates_holiday_and_partial_days() {
        let i = insight("codex", window("primary", 0.25, "2026-10-13T12:00:00-03:00", Some(604800)),
            "2026-10-09T18:00:00-03:00", &calendar());
        let p = i.period.unwrap();
        assert_eq!(p.start_ms, Some(epoch("2026-10-06T12:00:00-03:00")));
        let w = i.work.unwrap();
        near(w.cycle_total_days.unwrap(), 4.0);
        near(w.cycle_elapsed_days.unwrap(), 3.25);
        near(w.remaining_days, 0.75);
        near(w.elapsed_percent.unwrap(), 81.25);
        near(w.used_minus_work_pp.unwrap(), -56.25);
        let b = i.budget.unwrap();
        near(b.per_remaining_workday, 100.0);
        near(b.week_remaining_workdays, 0.25);
        near(b.week_remaining_budget, 25.0);
        near(b.week_reference_budget.unwrap(), 87.5);
        let tuesday = insight("codex", window("primary", 0.25, "2026-10-13T12:00:00-03:00", Some(604800)),
            "2026-10-13T06:00:00-03:00", &calendar());
        near(tuesday.budget.unwrap().week_remaining_budget, 75.0);
    }

    #[test]
    fn real_claude_and_codex_weekly_shapes_use_their_own_reset_times() {
        let claude = insight("claude", window("weekly_all", 0.79, "2026-10-14T12:59:59.703-03:00", None),
            "2026-10-12T08:38:00-03:00", &calendar());
        assert_eq!(claude.period.as_ref().unwrap().basis, PeriodBasis::OfficialWeekly);
        assert_eq!(claude.period.unwrap().start_ms, Some(epoch("2026-10-07T12:59:59.703-03:00")));
        near(claude.official_remaining_percent.unwrap(), 21.0);
        let codex = insight("codex", window("primary", 0.02, "2026-10-19T07:39:47-03:00", Some(604800)),
            "2026-10-12T08:38:00-03:00", &calendar());
        near(codex.work.as_ref().unwrap().cycle_elapsed_days.unwrap(), 0.0);
        near(codex.work.unwrap().used_minus_work_pp.unwrap(), 2.0);
        assert_eq!(codex.budget.unwrap().unit, BudgetUnit::Percent);
    }

    #[test]
    fn real_kiro_plan_preserves_explicit_balance_and_calendar_month() {
        let mut w = window("credits", 0.016558, "2026-11-01T00:00:00Z", None);
        w.credits = Some(CreditAmounts { consumed: 82.79, allowance: 5000.0 });
        w.credit_scope = Some(CreditScope::Plan);
        let i = insight("kiro", w, "2026-10-12T08:38:00-03:00", &calendar());
        let p = i.period.unwrap();
        assert_eq!(p.start_ms, Some(epoch("2026-10-01T00:00:00Z")));
        assert_eq!(p.duration_seconds, Some(31 * 86400));
        assert_eq!(p.basis, PeriodBasis::OfficialCalendarMonth);
        near(i.credits.as_ref().unwrap().remaining, 4917.21);
        near(i.official_used_percent.unwrap(), 1.6558);
        let work = i.work.unwrap();
        near(work.cycle_total_days.unwrap(), 21.125);
        near(work.cycle_elapsed_days.unwrap(), 7.125);
        near(work.remaining_days, 14.0);
        let b = i.budget.unwrap();
        assert_eq!(b.unit, BudgetUnit::Credits);
        near(b.per_remaining_workday, 4917.21 / 14.0);
        near(b.week_remaining_workdays, 4.0);
        near(b.week_remaining_budget, 4917.21 * 4.0 / 14.0);
    }

    #[test]
    fn leap_and_nonleap_february_are_not_fixed_thirty_day_cycles() {
        for (year, days) in [(2027, 28), (2028, 29)] {
            let mut w = window("credits", 0.5, &format!("{year}-03-01T00:00:00Z"), None);
            w.credit_scope = Some(CreditScope::Plan);
            w.credits = Some(CreditAmounts { consumed: 50.0, allowance: 100.0 });
            let i = insight("kiro", w, &format!("{year}-02-10T00:00:00Z"), &calendar());
            assert_eq!(i.period.unwrap().duration_seconds, Some(days * 86400));
        }
    }

    #[test]
    fn unknown_start_still_allows_remaining_percent_budget_without_cycle_history() {
        let i = insight("codex", window("secondary", 0.25, "2026-10-13T12:00:00-03:00", None),
            "2026-10-13T06:00:00-03:00", &calendar());
        assert_eq!(i.period.unwrap().start_ms, None);
        let w = i.work.unwrap();
        assert_eq!(w.cycle_total_days, None);
        assert_eq!(w.used_minus_work_pp, None);
        near(w.remaining_days, 0.25);
        let b = i.budget.unwrap();
        near(b.per_remaining_workday, 300.0);
        assert_eq!(b.reference_per_cycle_workday, None);
        assert_eq!(b.week_reference_budget, None);
        assert!(i.reasons.contains(&InsightReason::UnknownStart));
    }

    #[test]
    fn manual_working_override_and_partial_coverage_are_explicit() {
        let mut c = calendar();
        c.source_coverage.municipal = false;
        c.overrides.push(CalendarOverride { date: "2026-10-12".into(), name: "Work".into(), nonworking: false });
        c.overrides.push(CalendarOverride { date: "2026-10-11".into(), name: "Extra".into(), nonworking: false });
        let i = insight("codex", window("primary", 0.25, "2026-10-13T12:00:00-03:00", Some(604800)),
            "2026-10-12T12:00:00-03:00", &c);
        near(i.work.as_ref().unwrap().cycle_total_days.unwrap(), 5.0);
        near(i.work.as_ref().unwrap().remaining_days, 1.0);
        assert!(i.work.unwrap().calendar_partial);
        assert!(i.reasons.contains(&InsightReason::CalendarPartial));
        c.covered_years.clear();
        let unavailable = insight("codex", window("primary", 0.25, "2026-10-13T12:00:00-03:00", Some(604800)),
            "2026-10-12T12:00:00-03:00", &c);
        assert!(unavailable.work.is_none());
        assert!(unavailable.budget.is_none());
        assert!(unavailable.reasons.contains(&InsightReason::CalendarOutsideCoverage));
    }

    #[test]
    fn configured_offset_controls_partial_new_year_dates_not_the_host_timezone() {
        let mut c = calendar();
        c.holidays.push(Holiday { date: "2027-01-01".into(), name: "New year".into(), source: "fixture".into() });
        let w = window("primary", 0.0, "2027-01-06T00:00:00-03:00", Some(604800));
        let i = insight("codex", w.clone(), "2027-01-04T00:00:00-03:00", &c);
        near(i.work.as_ref().unwrap().cycle_total_days.unwrap(), 4.0);
        near(i.work.unwrap().cycle_elapsed_days.unwrap(), 2.0);
        c.utc_offset_minutes = 0;
        let utc = insight("codex", w, "2027-01-03T23:00:00-03:00", &c);
        near(utc.work.unwrap().cycle_elapsed_days.unwrap(), 1.0 + 23.0 / 24.0);
    }

    #[test]
    fn unknown_claude_kinds_and_nonmonthly_kiro_resets_do_not_invent_a_start() {
        let unknown = insight("claude", window("unknown_weekly_offer", 0.0, "2026-10-14T12:00:00-03:00", None),
            "2026-10-13T12:00:00-03:00", &calendar());
        assert_eq!(unknown.period.unwrap().basis, PeriodBasis::ResetOnly);
        near(unknown.official_remaining_percent.unwrap(), 100.0);
        assert!(unknown.work.unwrap().used_minus_work_pp.is_none());
        let mut w = window("credits", 0.5, "2026-11-15T00:00:00Z", Some(30 * 86400));
        w.credit_scope = Some(CreditScope::Plan);
        w.credits = Some(CreditAmounts { consumed: 50.0, allowance: 100.0 });
        let i = insight("kiro", w, "2026-10-12T12:00:00-03:00", &calendar());
        assert_eq!(i.period.unwrap().start_ms, None);
        assert!(i.work.unwrap().cycle_total_days.is_none());
        assert!(i.budget.unwrap().week_reference_budget.is_none());
        assert!(i.reasons.contains(&InsightReason::UnknownStart));
    }

    #[test]
    fn missing_calendar_and_invalid_credit_pairs_preserve_unknown_values() {
        let now = epoch("2026-10-12T12:00:00-03:00");
        let s = UsageSnapshot { status: "ok".into(), fetched_at: now,
            windows: vec![window("primary", 0.25, "2026-10-19T12:00:00-03:00", Some(604800))],
            ..Default::default() };
        let i = compute_provider_insights("codex", &s, None, now).windows.remove(0);
        assert!(i.period.is_some() && i.work.is_none() && i.budget.is_none());
        assert!(i.reasons.contains(&InsightReason::CalendarMissing));
        let mut w = window("credits", 0.5, "2026-11-01T00:00:00Z", None);
        w.credit_scope = Some(CreditScope::Plan);
        w.credits = Some(CreditAmounts { consumed: f64::NAN, allowance: 100.0 });
        let i = insight("kiro", w, "2026-10-12T12:00:00-03:00", &calendar());
        assert!(i.credits.is_none() && i.budget.is_none());
        assert!(i.reasons.contains(&InsightReason::MissingReading));
    }

    #[test]
    fn no_business_time_and_no_remaining_business_time_do_not_invent_zero_pace() {
        let weekend = insight("claude", window("session", 0.0, "2026-10-11T12:00:00-03:00", None),
            "2026-10-11T10:00:00-03:00", &calendar());
        assert_eq!(weekend.period.unwrap().basis, PeriodBasis::OfficialSession);
        assert!(weekend.work.is_none());
        assert!(weekend.budget.is_none());
        assert_eq!(weekend.reasons, vec![InsightReason::NoWorkdays]);
        let friday_reset = insight("codex", window("primary", 0.5, "2026-10-11T12:00:00-03:00", Some(604800)),
            "2026-10-10T12:00:00-03:00", &calendar());
        assert!(friday_reset.work.is_some());
        assert!(friday_reset.budget.is_none());
        assert!(friday_reset.reasons.contains(&InsightReason::NoRemainingWorkdays));
        near(friday_reset.official_remaining_percent.unwrap(), 50.0);
    }

    #[test]
    fn stale_missing_and_expired_reset_retain_only_reported_balance() {
        let now = epoch("2026-10-12T12:00:00-03:00");
        let w = window("primary", 0.25, "2026-10-19T12:00:00-03:00", Some(604800));
        for (status, fetched_at) in [("stale", now), ("ok", now - FRESH_FOR_MS - 1), ("ok", now + 1), ("ok", 0)] {
            let s = UsageSnapshot { status: status.into(), fetched_at, windows: vec![w.clone()], ..Default::default() };
            let i = compute_provider_insights("codex", &s, Some(&calendar()), now).windows.remove(0);
            assert!(!i.fresh);
            assert!(i.period.is_none() && i.work.is_none() && i.budget.is_none());
            near(i.official_remaining_percent.unwrap(), 75.0);
        }
        let expired = insight("codex", w, "2026-10-19T12:00:00-03:00", &calendar());
        assert_eq!(expired.reasons, vec![InsightReason::ResetExpired]);
        let missing = insight("codex", LimitWindow { used: 0.25, ..Default::default() },
            "2026-10-12T12:00:00-03:00", &calendar());
        assert_eq!(missing.reasons, vec![InsightReason::MissingReset]);
    }

    #[test]
    fn a_fresh_account_does_not_refresh_another_accounts_day_old_future_reset() {
        let now = epoch("2026-10-13T12:00:00-03:00");
        let a = LimitWindow { group: Some("A".into()), reading_fetched_at: Some(now), reading_status: Some("ok".into()),
            ..window("weekly_all", 0.25, "2026-10-16T12:00:00-03:00", None) };
        let b = LimitWindow { group: Some("B".into()), reading_fetched_at: Some(now - 86400000), reading_status: Some("ok".into()),
            ..window("weekly_all@work", 0.75, "2026-10-16T12:00:00-03:00", None) };
        let s = UsageSnapshot { status: "ok".into(), fetched_at: now, windows: vec![a.clone(), b], ..Default::default() };
        let i = compute_provider_insights("claude", &s, Some(&calendar()), now);
        assert!(i.windows[0].fresh && i.windows[0].budget.is_some());
        assert!(!i.windows[1].fresh);
        assert!(i.windows[1].period.is_none() && i.windows[1].work.is_none() && i.windows[1].budget.is_none());
        assert_eq!(i.windows[1].reasons, vec![InsightReason::Stale]);
        near(i.windows[1].official_used_percent.unwrap(), 75.0);
        assert_eq!(i.windows[1].window_index, 1);
        let mut partial = a.clone(); partial.reading_status = None;
        let mut future = a.clone(); future.reading_fetched_at = Some(now + 1);
        let mut stale = a; stale.reading_status = Some("stale".into());
        let s = UsageSnapshot { status: "ok".into(), fetched_at: now, windows: vec![partial, future, stale], ..Default::default() };
        let i = compute_provider_insights("claude", &s, Some(&calendar()), now);
        assert!(i.windows.iter().all(|w| !w.fresh && w.budget.is_none()));
        assert_eq!(i.windows[0].reasons, vec![InsightReason::MissingWindowFreshness]);
    }

    #[test]
    fn old_multiaccount_caches_do_not_borrow_aggregate_freshness() {
        let now = epoch("2026-10-13T12:00:00-03:00");
        let a = window("weekly_all", 0.25, "2026-10-16T12:00:00-03:00", None);
        let b = LimitWindow { group: Some("B".into()), ..window("weekly_all@work", 0.75, "2026-10-16T12:00:00-03:00", None) };
        let s = UsageSnapshot { status: "ok".into(), fetched_at: now, windows: vec![a.clone(), b], ..Default::default() };
        let i = compute_provider_insights("claude", &s, Some(&calendar()), now);
        assert!(i.windows.iter().all(|w| !w.fresh && w.budget.is_none()
            && w.reasons.contains(&InsightReason::MissingWindowFreshness)));
        let single = UsageSnapshot { windows: vec![a], ..s };
        assert!(compute_provider_insights("claude", &single, Some(&calendar()), now).windows[0].fresh);
    }

    #[test]
    fn a_stale_envelope_cannot_be_overridden_by_recent_window_metadata() {
        let now = epoch("2026-10-13T12:00:00-03:00");
        let w = LimitWindow { reading_fetched_at: Some(now), reading_status: Some("ok".into()),
            ..window("weekly_all@work", 0.25, "2026-10-16T12:00:00-03:00", None) };
        let mut s = UsageSnapshot { status: "stale".into(), fetched_at: now, windows: vec![w], ..Default::default() };
        let stale = compute_provider_insights("claude", &s, Some(&calendar()), now);
        assert!(!stale.windows[0].fresh);
        assert!(stale.windows[0].period.is_none() && stale.windows[0].work.is_none() && stale.windows[0].budget.is_none());
        assert_eq!(stale.windows[0].reasons, vec![InsightReason::Stale]);
        s.status = "ok".into();
        let current = compute_provider_insights("claude", &s, Some(&calendar()), now);
        assert!(current.windows[0].fresh && current.windows[0].budget.is_some());
        assert_eq!(current.windows[0].period.as_ref().unwrap().basis, PeriodBasis::OfficialWeekly);
        assert_eq!(current.windows[0].period.as_ref().unwrap().duration_seconds, Some(604800));
    }

    #[test]
    fn combined_unknown_bonus_and_overage_never_become_monthly_plan_budget() {
        for scope in [None, Some(CreditScope::Unknown), Some(CreditScope::Combined)] {
            let mut w = window("credits", 0.0, "2026-11-01T00:00:00Z", None);
            w.credit_scope = scope;
            w.credits = Some(CreditAmounts { consumed: 0.17, allowance: 50.0 });
            let i = insight("kiro", w, "2026-10-12T12:00:00-03:00", &calendar());
            near(i.official_used_percent.unwrap(), 0.0);
            near(i.credits.unwrap().remaining, 49.83);
            assert!(i.work.is_none() && i.budget.is_none());
            assert!(i.reasons.contains(&InsightReason::UnknownCreditScope));
        }
        for id in ["bonus", "overage"] {
            let mut w = window(id, 0.5, "2026-11-01T00:00:00Z", None);
            w.credits = Some(CreditAmounts { consumed: 5.0, allowance: 10.0 });
            let i = insight("kiro", w, "2026-10-12T12:00:00-03:00", &calendar());
            near(i.credits.unwrap().remaining, 5.0);
            assert!(i.period.is_none() && i.work.is_none() && i.budget.is_none());
            assert_eq!(i.reasons, vec![InsightReason::NonrenewingWallet]);
        }
    }

    #[test]
    fn nulls_order_indices_and_units_survive_serialization() {
        let now = epoch("2026-10-13T06:00:00-03:00");
        let first = window("primary", 0.25, "2026-10-13T12:00:00-03:00", None);
        let second = LimitWindow { id: "primary".into(), group: Some("Other account".into()), ..first.clone() };
        let s = UsageSnapshot { status: "ok".into(), fetched_at: now, windows: vec![first, second], ..Default::default() };
        let i = compute_provider_insights("codex", &s, Some(&calendar()), now);
        let v = serde_json::to_value(i).unwrap();
        assert_eq!(v["windows"][0]["window_index"], 0);
        assert_eq!(v["windows"][1]["window_index"], 1);
        assert!(v["windows"][0]["credits"].is_null());
        assert!(v["windows"][0]["period"]["start_ms"].is_null());
        assert!(v["windows"][0]["work"]["used_minus_work_pp"].is_null());
        assert_eq!(v["windows"][0]["budget"]["unit"], "percent");
        assert_eq!(v["windows"][1]["group"], "Other account");
    }

    #[test]
    fn invalid_numbers_extreme_times_and_bad_calendars_do_not_create_a_forecast() {
        for used in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            let i = insight("codex", window("primary", used, "2026-10-19T12:00:00-03:00", Some(604800)),
                "2026-10-12T12:00:00-03:00", &calendar());
            assert!(i.official_used_percent.is_none() && i.budget.is_none());
            assert_eq!(i.reasons, vec![InsightReason::InvalidReading]);
        }
        let mut c = calendar(); c.utc_offset_minutes = 900;
        let i = insight("codex", window("primary", 0.5, "2026-10-19T12:00:00-03:00", Some(604800)),
            "2026-10-12T12:00:00-03:00", &c);
        assert!(i.budget.is_none());
        assert!(i.reasons.contains(&InsightReason::CalendarMissing));
        let i = insight("codex", LimitWindow { resets_at: Some(u64::MAX), used: 0.25, ..Default::default() },
            "2026-10-12T12:00:00-03:00", &calendar());
        assert!(i.budget.is_none());
        assert!(i.reasons.contains(&InsightReason::InvalidPeriod));
    }
}
