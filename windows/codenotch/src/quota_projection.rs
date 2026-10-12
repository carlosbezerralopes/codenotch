//! Pure composition of official balances, bounded history and the shared calendar.
use crate::telemetry::quota_index::{series_key_for, QuotaHistoryRequest, QuotaHistoryResult, QuotaHistoryState};
use crate::usage::{CreditScope, LimitWindow, QuotaResetBasis, UsageSnapshot};
use crate::usage_insights::*;
use crate::work_calendar::CalendarSettings;
use std::collections::HashMap;

const DAY_MS: u64 = 86_400_000;

pub(crate) fn classify_window(provider: &str, id: &str, duration: Option<u64>, scope: Option<CreditScope>) -> (QuotaRole, PeriodKind) {
    let kind = id.split('@').next().unwrap_or(id);
    match provider {
        "claude" => match kind {
            "session" | "five_hour" => (QuotaRole::Session, PeriodKind::Session),
            "seven_day" | "weekly_all" | "weekly" => (QuotaRole::General, PeriodKind::Weekly),
            "weekly_scoped" | "seven_day_opus" | "weekly_opus" => (QuotaRole::Scoped, PeriodKind::Weekly),
            _ => (QuotaRole::Other, PeriodKind::Unknown),
        },
        "codex" => {
            let role = match kind {
                "primary" | "secondary" => QuotaRole::General,
                "spark" | "spark-secondary" | "code-review" | "code-review-secondary" => QuotaRole::Scoped,
                _ => return (QuotaRole::Other, PeriodKind::Unknown),
            };
            match duration {
                Some(d) if d > 0 && d < 86400 => (QuotaRole::Session, PeriodKind::Session),
                Some(d) if (28 * 86400..=366 * 86400).contains(&d) => (role, PeriodKind::Monthly),
                Some(d) if (86400..28 * 86400).contains(&d) => (role, PeriodKind::Weekly),
                _ => (role, PeriodKind::Unknown),
            }
        }
        "kiro" if kind == "credits" && scope == Some(CreditScope::Plan) => (QuotaRole::General, PeriodKind::Monthly),
        _ => (QuotaRole::Other, PeriodKind::Unknown),
    }
}

fn long_window(provider: &str, w: &LimitWindow, b: &WindowInsights) -> bool {
    let (role, kind) = classify_window(provider, &w.id, b.period.as_ref().and_then(|p| p.duration_seconds).or(w.duration_seconds), w.credit_scope);
    matches!(role, QuotaRole::General | QuotaRole::Scoped) && matches!(kind, PeriodKind::Weekly | PeriodKind::Monthly)
}

fn balance(provider: &str, w: &LimitWindow) -> Option<(BudgetUnit, f64, f64, f64)> {
    if !w.used.is_finite() || !(0.0..=1.0).contains(&w.used) || w.count.is_some() || w.derived { return None; }
    if provider == "kiro" {
        if w.id != "credits" || w.credit_scope != Some(CreditScope::Plan) { return None; }
        let c = w.credits.as_ref()?;
        if !c.consumed.is_finite() || c.consumed < 0.0 || !c.allowance.is_finite() || c.allowance <= 0.0 { return None; }
        Some((BudgetUnit::Credits, c.consumed, (c.allowance - c.consumed).max(0.0), c.allowance))
    } else if matches!(provider, "claude" | "codex") {
        Some((BudgetUnit::Percent, w.used * 100.0, (1.0 - w.used) * 100.0, 100.0))
    } else { None }
}

fn continuous_reset(w: &LimitWindow) -> bool {
    matches!(w.reset_basis, Some(QuotaResetBasis::Absolute | QuotaResetBasis::OfficialCalendarMonth))
}

pub(crate) fn history_requests(provider: &str, snapshot: &UsageSnapshot, base: &ProviderInsights,
    calendar: Option<&CalendarSettings>, now_ms: u64) -> Vec<QuotaHistoryRequest> {
    let Some(work) = calendar.and_then(|c| WorkingCalendar::new(c).ok()) else { return Vec::new(); };
    base.windows.iter().filter_map(|b| {
        let w = snapshot.windows.get(b.window_index)?;
        let (f, status) = window_origin(provider, snapshot, w)?;
        if !b.fresh || status != "ok" || f > now_ms || !long_window(provider, w, b) || !continuous_reset(w) || balance(provider, w).is_none() { return None; }
        let period = b.period.as_ref()?;
        let start = period.start_ms?;
        if start > f || period.reset_ms <= now_ms || work.days(start, f).ok()? < 1.0 { return None; }
        let from = work.timestamp_before_workdays(f, start, 3.0).ok()?;
        Some(QuotaHistoryRequest { window_index: b.window_index, series_key: series_key_for(provider, w, period)?, from_ms: from, to_ms: f })
    }).collect()
}

fn empty_projection(unit: BudgetUnit) -> QuotaProjection {
    QuotaProjection { state: ProjectionState::Unavailable, basis: None, unit,
        sampled_from_ms: None, sampled_to_ms: None, sample_count: 0, observed_workdays: None,
        coverage_fraction: None, rate_per_workday: None, rate_percent: None, exhaustion_at_ms: None,
        exhaustion_workdays: None, remaining_workdays: None, early_workdays: None,
        early_calendar_ms: None, margin_at_reset: None, extra_or_mixed_consumption: false, reasons: Vec::new() }
}

fn fail(p: &mut QuotaProjection, state: ProjectionState, reason: ProjectionReason) {
    p.state = state;
    p.rate_per_workday = None;
    p.rate_percent = None;
    p.exhaustion_at_ms = None;
    p.exhaustion_workdays = None;
    p.early_workdays = None;
    p.early_calendar_ms = None;
    p.margin_at_reset = None;
    if !p.reasons.contains(&reason) { p.reasons.push(reason); }
}

fn calendar_reason(reason: InsightReason) -> ProjectionReason {
    match reason { InsightReason::CalendarOutsideCoverage => ProjectionReason::CalendarOutsideCoverage,
        InsightReason::InvalidPeriod => ProjectionReason::InvalidPeriod, _ => ProjectionReason::CalendarMissing }
}

fn percent_of_allowance(unit: BudgetUnit, amount: Option<f64>, allowance: Option<f64>) -> Option<f64> {
    let amount = amount.filter(|n| n.is_finite() && *n >= 0.0)?;
    if unit == BudgetUnit::Percent { return Some(amount); }
    let allowance = allowance.filter(|n| n.is_finite() && *n > 0.0)?;
    let percent = amount / allowance * 100.0;
    percent.is_finite().then_some(percent)
}

fn display(unit: BudgetUnit, remaining: Option<f64>, allowance: Option<f64>, f: Option<u64>, reset: Option<u64>,
    useful: Option<f64>, session: bool, current: bool) -> DisplayBudget {
    let available = current && remaining.is_some() && f.is_some() && reset.is_some();
    let mode = if !available { DisplayBudgetMode::Unavailable }
        else if session { DisplayBudgetMode::SessionQuotaReset }
        else { match useful {
            Some(d) if d >= 1.0 => DisplayBudgetMode::PerWorkday,
            Some(d) if d > 0.0 => DisplayBudgetMode::UntilReset,
            Some(0.0) => DisplayBudgetMode::ExtraDaysUntilReset,
            _ => DisplayBudgetMode::Unavailable,
        } };
    let amount = match mode {
        DisplayBudgetMode::PerWorkday => remaining.zip(useful).map(|(m, d)| m / d),
        DisplayBudgetMode::SessionQuotaReset | DisplayBudgetMode::UntilReset | DisplayBudgetMode::ExtraDaysUntilReset => remaining,
        DisplayBudgetMode::Unavailable => None,
    }.filter(|n| n.is_finite());
    DisplayBudget { mode, unit, amount, amount_percent: percent_of_allowance(unit, amount, allowance), horizon_start_ms: f, horizon_end_ms: reset,
        workdays_remaining: useful, as_of_ms: f, is_current: available && mode != DisplayBudgetMode::Unavailable }
}

/// Reconstruct only the period at the actual origin for qualified stale references.
fn origin_base(provider: &str, snapshot: &UsageSnapshot, w: &LimitWindow, f: u64,
    calendar: Option<&CalendarSettings>) -> WindowInsights {
    let mut s = snapshot.clone();
    s.status = "ok".into(); s.fetched_at = f;
    let mut w = w.clone();
    w.reading_fetched_at = Some(f); w.reading_status = Some("ok".into());
    s.windows = vec![w];
    compute_provider_insights(provider, &s, calendar, f).windows.remove(0)
}

pub(crate) fn attach_projections(provider: &str, snapshot: &UsageSnapshot, base: &mut ProviderInsights,
    calendar: Option<&CalendarSettings>, history: &HashMap<usize, QuotaHistoryResult>, now_ms: u64) {
    let requests: HashMap<_, _> = history_requests(provider, snapshot, base, calendar, now_ms)
        .into_iter().map(|r| (r.window_index, r)).collect();
    let work = calendar.and_then(|c| WorkingCalendar::new(c).ok());
    for b in &mut base.windows {
        b.projection = None; b.display_budget = None; b.last_reading_reference = None;
        let Some(w) = snapshot.windows.get(b.window_index) else { continue; };
        let (role, kind) = classify_window(provider, &w.id,
            b.period.as_ref().and_then(|p| p.duration_seconds).or(w.duration_seconds), w.credit_scope);
        b.quota_role = Some(role); b.period_kind = Some(kind);
        if role == QuotaRole::Other { continue; }
        let unit = if provider == "kiro" { BudgetUnit::Credits } else { BudgetUnit::Percent };
        let amounts = balance(provider, w);
        let origin = window_origin(provider, snapshot, w).map(|(at, _)| at).filter(|at| *at > 0 && *at <= now_ms);
        let reset = w.resets_at.filter(|r| *r > now_ms);
        let useful = origin.zip(reset).and_then(|(f, r)| work.as_ref()?.days(f, r).ok());
        b.display_budget = Some(display(unit, amounts.map(|a| a.2), amounts.map(|a| a.3), origin, reset, useful, role == QuotaRole::Session, b.fresh));
        if !b.fresh {
            if let (Some(f), Some((_, _, m, allowance))) = (origin, amounts) {
                let reference = origin_base(provider, snapshot, w, f, calendar);
                let ref_useful = if reset.is_some() && reference.period.is_some() { useful } else { None };
                let mut ref_display = display(unit, Some(m), Some(allowance), Some(f), reset, ref_useful, role == QuotaRole::Session, true);
                ref_display.is_current = false;
                b.last_reading_reference = Some(LastReadingReference { as_of_ms: f, age_ms: now_ms - f,
                    official_remaining: Some(m), unit, per_remaining_workday_as_of: ref_useful.filter(|d| *d > 0.0 && role != QuotaRole::Session).map(|d| m / d).filter(|n| n.is_finite()),
                    reset_ms: w.resets_at, display_budget: reset.map(|_| ref_display), previous_cycle: w.resets_at.is_some_and(|r| r <= now_ms) });
            }
            if role != QuotaRole::Session {
                let mut p = empty_projection(unit); fail(&mut p, ProjectionState::Unavailable, ProjectionReason::Stale); b.projection = Some(p);
            }
            continue;
        }
        if role == QuotaRole::Session { continue; }
        let mut p = empty_projection(unit);
        p.remaining_workdays = useful;
        let Some((_, consumed, remaining, allowance)) = amounts else {
            fail(&mut p, ProjectionState::Unavailable, ProjectionReason::InvalidReading); b.projection = Some(p); continue;
        };
        let Some(f) = origin else { fail(&mut p, ProjectionState::Unavailable, ProjectionReason::MissingOrigin); b.projection = Some(p); continue; };
        let Some(reset) = reset else {
            let reason = if w.resets_at.is_some() { ProjectionReason::ResetExpired } else { ProjectionReason::MissingReset };
            fail(&mut p, ProjectionState::Unavailable, reason); b.projection = Some(p); continue;
        };
        // Official zero is meaningful even on a holiday and without historical evidence.
        if remaining == 0.0 {
            p.state = ProjectionState::Exhausted; p.sampled_to_ms = Some(f); p.exhaustion_at_ms = Some(f);
            p.exhaustion_workdays = Some(0.0); p.early_workdays = useful; p.early_calendar_ms = Some(reset - f);
            b.projection = Some(p); continue;
        }
        if !long_window(provider, w, b) {
            fail(&mut p, ProjectionState::Unavailable, ProjectionReason::UnsupportedWindow); b.projection = Some(p); continue;
        }
        let result = compute_rate_and_forecast(provider, w, b, f, reset, consumed, remaining, allowance,
            calendar, requests.get(&b.window_index), history.get(&b.window_index), now_ms, &mut p);
        if let Err(reason) = result { fail(&mut p, ProjectionState::Unavailable, reason); }
        b.projection = Some(p);
    }
}

#[allow(clippy::too_many_arguments)]
fn compute_rate_and_forecast(provider: &str, w: &LimitWindow, b: &WindowInsights, f: u64, reset: u64,
    consumed: f64, remaining: f64, allowance: f64, calendar: Option<&CalendarSettings>,
    request: Option<&QuotaHistoryRequest>, history: Option<&QuotaHistoryResult>, now: u64,
    p: &mut QuotaProjection) -> Result<(), ProjectionReason> {
    let period = b.period.as_ref().ok_or(ProjectionReason::InvalidPeriod)?;
    let start = period.start_ms.filter(|s| *s <= f).ok_or(ProjectionReason::UnknownStart)?;
    let settings = calendar.ok_or(ProjectionReason::CalendarMissing)?;
    let work = WorkingCalendar::new(settings).map_err(calendar_reason)?;
    let elapsed = work.days(start, f).map_err(calendar_reason)?;
    let remaining_days = work.days(f, reset).map_err(calendar_reason)?;
    p.remaining_workdays = Some(remaining_days);
    if !(settings.source_coverage.national && settings.source_coverage.state && settings.source_coverage.municipal) { p.reasons.push(ProjectionReason::CalendarPartial); }
    if elapsed < 1.0 {
        fail(p, ProjectionState::InsufficientWorkTime, ProjectionReason::InsufficientWorkTime); return Ok(());
    }
    if remaining_days == 0.0 {
        fail(p, ProjectionState::NoRemainingWorkTime, ProjectionReason::NoRemainingWorkTime); return Ok(());
    }
    let rate = if let Some(request) = request {
        let Some(h) = history else { fail(p, ProjectionState::HistoryIncomplete, ProjectionReason::HistoryIncomplete); return Ok(()); };
        match h.state {
            QuotaHistoryState::Truncated => { fail(p, ProjectionState::HistoryTruncated, ProjectionReason::HistoryTruncated); return Ok(()); }
            QuotaHistoryState::Conflict => { fail(p, ProjectionState::HistoryIncomplete, ProjectionReason::HistoryConflict); return Ok(()); }
            QuotaHistoryState::Warming => { fail(p, ProjectionState::HistoryIncomplete, ProjectionReason::HistoryWarming); return Ok(()); }
            QuotaHistoryState::Incomplete => { fail(p, ProjectionState::HistoryIncomplete, ProjectionReason::HistoryIncomplete); return Ok(()); }
            QuotaHistoryState::Unavailable => { fail(p, ProjectionState::Unavailable, ProjectionReason::HistoryUnavailable); return Ok(()); }
            QuotaHistoryState::Complete => {}
        }
        if !h.complete_horizon { fail(p, ProjectionState::HistoryIncomplete, ProjectionReason::HistoryIncomplete); return Ok(()); }
        observed_rate(h, request, consumed, allowance, &work, p)?
    } else {
        if w.quota_account_key.is_none() || w.quota_bucket_key.is_none() { p.reasons.push(ProjectionReason::IdentityUnknown); }
        else if !continuous_reset(w) { p.reasons.push(ProjectionReason::RelativeReset); }
        else if series_key_for(provider, w, period).is_none() { return Err(ProjectionReason::InvalidPeriod); }
        None
    };
    let (c, d) = match rate {
        Some(rate) => rate,
        None if matches!(p.state, ProjectionState::Unavailable) && p.reasons.iter().all(|r| matches!(r, ProjectionReason::CalendarPartial | ProjectionReason::IdentityUnknown | ProjectionReason::RelativeReset | ProjectionReason::InsufficientHistory)) => {
            if consumed <= 0.0 {
                let identity = p.reasons.contains(&ProjectionReason::IdentityUnknown);
                fail(p, if identity { ProjectionState::IdentityUnknown } else { ProjectionState::InsufficientHistory }, ProjectionReason::InsufficientHistory);
                return Ok(());
            }
            p.basis = Some(ProjectionBasis::CycleMean); p.sampled_from_ms = Some(start); p.sampled_to_ms = Some(f);
            p.sample_count = 1; p.observed_workdays = Some(elapsed);
            p.reasons.extend([ProjectionReason::CycleMeanIncludesExtras, ProjectionReason::NoRecentRateMeasured]);
            (consumed, elapsed)
        }
        None => return Ok(()),
    };
    let rate = c / d;
    if !rate.is_finite() || rate <= 0.0 { return Err(ProjectionReason::InvalidReading); }
    if !(remaining * d).is_finite() || !(c * remaining_days).is_finite() { return Err(ProjectionReason::InvalidReading); }
    p.rate_per_workday = Some(rate);
    p.rate_percent = percent_of_allowance(p.unit, Some(rate), Some(allowance));
    // Cross products decide coverage; display rounding never determines the state.
    if remaining * d >= c * remaining_days {
        p.state = ProjectionState::CoversReset;
        p.margin_at_reset = Some((remaining - rate * remaining_days).max(0.0));
        return Ok(());
    }
    let needed_days = remaining * d / c;
    let e = work.timestamp_after_workdays(f, reset, needed_days).map_err(calendar_reason)?.ok_or(ProjectionReason::InvalidPeriod)?;
    if e >= reset {
        p.state = ProjectionState::CoversReset; p.margin_at_reset = Some(0.0); return Ok(());
    }
    p.state = if e <= now { ProjectionState::ProjectedDue } else { ProjectionState::BeforeReset };
    p.exhaustion_at_ms = Some(e); p.exhaustion_workdays = Some(work.days(f, e).map_err(calendar_reason)?);
    p.early_workdays = Some(work.days(e, reset).map_err(calendar_reason)?); p.early_calendar_ms = Some(reset - e);
    Ok(())
}

fn observed_rate(h: &QuotaHistoryResult, request: &QuotaHistoryRequest, consumed: f64, allowance: f64,
    work: &WorkingCalendar<'_>, p: &mut QuotaProjection) -> Result<Option<(f64, f64)>, ProjectionReason> {
    let mut samples: Vec<_> = h.samples.iter().filter(|s| s.at_ms >= request.from_ms && s.at_ms <= request.to_ms
        && h.segment_started_at_ms.is_none_or(|start| s.at_ms >= start)).collect();
    if samples.iter().any(|s| s.series_key != request.series_key || !s.value.is_finite() || s.value < 0.0
        || (s.unit == BudgetUnit::Percent && s.value > 100.0)
        || s.unit != p.unit || s.allowance != allowance || !s.allowance.is_finite()) { return Err(ProjectionReason::InvalidReading); }
    samples.sort_by_key(|s| s.at_ms);
    let mut unique = Vec::new();
    for s in samples {
        if let Some(previous) = unique.last() {
            let previous: &&crate::telemetry::quota_index::QuotaRateSample = previous;
            if previous.at_ms == s.at_ms {
                if previous.value != s.value { fail(p, ProjectionState::HistoryIncomplete, ProjectionReason::HistoryConflict); return Ok(None); }
                continue;
            }
        }
        unique.push(s);
    }
    // An index result from a different snapshot cannot anchor this forecast.
    if unique.last().is_none_or(|s| s.at_ms != request.to_ms || s.value != consumed) {
        fail(p, ProjectionState::HistoryIncomplete, ProjectionReason::HistoryIncomplete); return Ok(None);
    }
    let segment = unique.windows(2).rposition(|pair| pair[1].value < pair[0].value).map(|index| index + 1).unwrap_or(0);
    let unique = &unique[segment..];
    p.sample_count = unique.len(); p.sampled_from_ms = unique.first().map(|s| s.at_ms); p.sampled_to_ms = unique.last().map(|s| s.at_ms);
    if unique.len() < 2 {
        if h.revised || segment > 0 { fail(p, ProjectionState::InsufficientHistory, ProjectionReason::HistoryRevision); }
        else { p.reasons.push(ProjectionReason::InsufficientHistory); }
        return Ok(None);
    }
    let mut duration_ms = 0u64;
    let mut consumed_delta = 0.0;
    for pair in unique.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let useful_ms: u64 = work.useful_segments(a.at_ms, b.at_ms).map_err(calendar_reason)?.iter().map(|(l, r)| r - l).sum();
        let delta = b.value - a.value;
        if useful_ms == b.at_ms - a.at_ms { duration_ms += useful_ms; consumed_delta += delta; }
        else if delta > 0.0 { p.extra_or_mixed_consumption = true; }
    }
    if p.extra_or_mixed_consumption { p.reasons.push(ProjectionReason::MixedOrExtraConsumption); }
    let whole_ms: u64 = work.useful_segments(unique[0].at_ms, request.to_ms).map_err(calendar_reason)?.iter().map(|(l, r)| r - l).sum();
    let d = duration_ms as f64 / DAY_MS as f64;
    p.observed_workdays = Some(d);
    p.coverage_fraction = (whole_ms > 0).then(|| duration_ms as f64 / whole_ms as f64);
    if duration_ms < DAY_MS {
        fail(p, ProjectionState::InsufficientWorkTime, if h.revised || segment > 0 { ProjectionReason::HistoryRevision } else { ProjectionReason::InsufficientWorkTime }); return Ok(None);
    }
    if whole_ms == 0 || (duration_ms as u128) * 2 < whole_ms as u128 {
        fail(p, ProjectionState::InsufficientCoverage, ProjectionReason::InsufficientCoverage); return Ok(None);
    }
    if consumed_delta <= 0.0 { fail(p, ProjectionState::NoObservedIncrease, ProjectionReason::NoObservedIncrease); return Ok(None); }
    p.basis = Some(ProjectionBasis::ObservedRecent);
    Ok(Some((consumed_delta, d)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::telemetry::quota_index::QuotaRateSample;
    use crate::usage::{CreditAmounts, QuotaReadingTransport};
    use crate::work_calendar::{CalendarOverride, Holiday, SourceCoverage};

    fn epoch(raw: &str) -> u64 { chrono::DateTime::parse_from_rfc3339(raw).unwrap().timestamp_millis() as u64 }
    fn calendar() -> CalendarSettings {
        CalendarSettings { utc_offset_minutes: 0, covered_years: vec![2026, 2027, 2028],
            source_coverage: SourceCoverage { national: true, state: true, municipal: true, ..Default::default() },
            ..Default::default() }
    }
    fn window(used: f64, reset: &str) -> LimitWindow {
        LimitWindow { id: "primary".into(), used, resets_at: Some(epoch(reset)), duration_seconds: Some(7 * 86400),
            quota_account_key: Some("a".repeat(64)), quota_bucket_key: Some("b".repeat(64)),
            reading_transport: Some(QuotaReadingTransport::CodexHttp), reset_basis: Some(QuotaResetBasis::Absolute), ..Default::default() }
    }
    fn run(w: LimitWindow, f: &str, now: &str, cal: &CalendarSettings,
        readings: Option<Vec<(&str, f64)>>, state: QuotaHistoryState) -> WindowInsights {
        let f = epoch(f); let now = epoch(now);
        let s = UsageSnapshot { status: "ok".into(), fetched_at: f, windows: vec![w], ..Default::default() };
        let mut b = compute_provider_insights("codex", &s, Some(cal), now);
        let mut h = HashMap::new();
        if let Some(readings) = readings {
            let period = b.windows[0].period.as_ref().unwrap();
            let key = series_key_for("codex", &s.windows[0], period).unwrap();
            h.insert(0, QuotaHistoryResult { state, samples: readings.into_iter().map(|(at, value)| QuotaRateSample {
                series_key: key.clone(), at_ms: epoch(at), value, allowance: 100.0, unit: BudgetUnit::Percent,
            }).collect(), complete_horizon: true, segment_started_at_ms: None, revised: false });
        }
        attach_projections("codex", &s, &mut b, Some(cal), &h, now);
        b.windows.remove(0)
    }
    fn near(a: f64, b: f64) { assert!((a - b).abs() < 1e-8, "{a} != {b}"); }

    #[test]
    fn t01_observed_rate_and_equality_cover_reset() {
        for used in [0.3, 0.4] {
            let i = run(window(used, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(),
                Some(vec![("2026-10-13T00:00:00Z", used * 100.0 - 20.0), ("2026-10-14T00:00:00Z", used * 100.0)]), QuotaHistoryState::Complete);
            let p = i.projection.unwrap(); assert_eq!(p.state, ProjectionState::CoversReset);
            assert_eq!(p.basis, Some(ProjectionBasis::ObservedRecent)); near(p.rate_per_workday.unwrap(), 20.0);
            near(p.remaining_workdays.unwrap(), 3.0); assert!(p.exhaustion_at_ms.is_none());
        }
    }

    #[test]
    fn t02_exhaustion_and_early_useful_days_are_exact() {
        let p = run(window(0.8, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(),
            Some(vec![("2026-10-13T00:00:00Z", 60.0), ("2026-10-14T00:00:00Z", 80.0)]), QuotaHistoryState::Complete).projection.unwrap();
        assert_eq!(p.state, ProjectionState::BeforeReset);
        assert_eq!(p.exhaustion_at_ms, Some(epoch("2026-10-15T00:00:00Z")));
        near(p.early_workdays.unwrap(), 2.0); assert_eq!(p.early_calendar_ms, Some(2 * DAY_MS));
    }

    #[test]
    fn t03_inverse_calendar_skips_weekend_and_clips_horizon() {
        let c = calendar(); let work = WorkingCalendar::new(&c).unwrap();
        let f = epoch("2026-10-16T12:00:00Z"); let reset = epoch("2026-10-20T12:00:00Z");
        let e = work.timestamp_after_workdays(f, reset, 1.0).unwrap().unwrap();
        assert_eq!(e, epoch("2026-10-19T12:00:00Z")); near(work.days(e, reset).unwrap(), 1.0);
        let h = work.timestamp_before_workdays(epoch("2026-10-19T12:00:00Z"), epoch("2026-10-12T00:00:00Z"), 3.0).unwrap();
        assert_eq!(h, epoch("2026-10-14T12:00:00Z"));
    }

    #[test]
    fn t04_real_codex_holiday_preserves_daily_balance() {
        let mut c = calendar(); c.utc_offset_minutes = -180;
        c.holidays.push(Holiday { date: "2026-10-12".into(), name: "Holiday".into(), source: "fixture".into() });
        let i = run(window(0.15, "2026-10-19T07:39:47-03:00"), "2026-10-12T08:38:00-03:00", "2026-10-12T08:38:00-03:00", &c, None, QuotaHistoryState::Complete);
        assert_eq!(i.projection.unwrap().state, ProjectionState::InsufficientWorkTime);
        let base = i.budget.unwrap().per_remaining_workday;
        near(base, 85.0 / (4.0 + (7.0 * 3600.0 + 39.0 * 60.0 + 47.0) / 86400.0));
        near(i.display_budget.unwrap().amount.unwrap(), base);
    }

    #[test]
    fn t05_official_zero_precedes_calendar_and_history() {
        let f = epoch("2026-10-12T08:38:00Z");
        let s = UsageSnapshot { status: "ok".into(), fetched_at: f, windows: vec![window(1.0, "2026-10-19T07:39:00Z")], ..Default::default() };
        let mut b = compute_provider_insights("codex", &s, None, f);
        attach_projections("codex", &s, &mut b, None, &HashMap::new(), f);
        let p = b.windows[0].projection.as_ref().unwrap();
        assert_eq!(p.state, ProjectionState::Exhausted); assert_eq!(p.exhaustion_at_ms, Some(f)); assert!(p.rate_per_workday.is_none());
    }

    #[test]
    fn t06_mixed_pair_is_excluded_without_delta_imputation() {
        let p = run(window(0.6, "2026-10-21T00:00:00Z"), "2026-10-19T12:00:00Z", "2026-10-19T12:00:00Z", &calendar(),
            Some(vec![("2026-10-15T12:00:00Z", 10.0), ("2026-10-16T12:00:00Z", 30.0), ("2026-10-19T12:00:00Z", 60.0)]), QuotaHistoryState::Complete).projection.unwrap();
        assert_eq!(p.basis, Some(ProjectionBasis::ObservedRecent)); near(p.observed_workdays.unwrap(), 1.0);
        near(p.coverage_fraction.unwrap(), 0.5); near(p.rate_per_workday.unwrap(), 20.0);
        assert!(p.extra_or_mixed_consumption);
    }

    #[test]
    fn t07_useful_day_minimum_is_not_rounded_up() {
        let p = run(window(0.3, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(),
            Some(vec![("2026-10-13T00:00:00.001Z", 10.0), ("2026-10-14T00:00:00Z", 30.0)]), QuotaHistoryState::Complete).projection.unwrap();
        assert_eq!(p.state, ProjectionState::InsufficientWorkTime); assert!(p.rate_per_workday.is_none());
    }

    #[test]
    fn t08_quantized_flat_history_does_not_promise_coverage_or_fallback() {
        let p = run(window(0.3, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(),
            Some(vec![("2026-10-13T00:00:00Z", 30.0), ("2026-10-14T00:00:00Z", 30.0)]), QuotaHistoryState::Complete).projection.unwrap();
        assert_eq!(p.state, ProjectionState::NoObservedIncrease); assert!(p.rate_per_workday.is_none() && p.basis.is_none());
    }

    #[test]
    fn t09_cycle_mean_uses_origin_not_later_hover_and_is_qualified() {
        let mut w = window(0.3, "2026-10-17T00:00:00Z"); w.quota_account_key = None;
        let i = run(w, "2026-10-14T00:00:00Z", "2026-10-14T00:10:00Z", &calendar(), None, QuotaHistoryState::Complete);
        let p = i.projection.unwrap(); assert_eq!(p.basis, Some(ProjectionBasis::CycleMean));
        near(p.rate_per_workday.unwrap(), 15.0); near(p.observed_workdays.unwrap(), 2.0);
        assert!(p.reasons.contains(&ProjectionReason::CycleMeanIncludesExtras));
    }

    #[test]
    fn t13_duplicate_origins_dedup_and_conflicts_fail_closed() {
        for (value, expected) in [(30.0, ProjectionState::CoversReset), (31.0, ProjectionState::HistoryIncomplete)] {
            let p = run(window(0.3, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(),
                Some(vec![("2026-10-13T00:00:00Z", 10.0), ("2026-10-14T00:00:00Z", 30.0), ("2026-10-14T00:00:00Z", value)]), QuotaHistoryState::Complete).projection.unwrap();
            assert_eq!(p.state, expected); if value == 30.0 { assert_eq!(p.sample_count, 2); } else { assert!(p.rate_per_workday.is_none()); }
        }
    }

    #[test]
    fn t14_negative_revision_requires_a_new_segment_minimum() {
        let p = run(window(0.2, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(),
            Some(vec![("2026-10-13T00:00:00Z", 30.0), ("2026-10-14T00:00:00Z", 20.0)]), QuotaHistoryState::Complete).projection.unwrap();
        assert_eq!(p.state, ProjectionState::InsufficientHistory); assert!(p.reasons.contains(&ProjectionReason::HistoryRevision)); assert!(p.basis.is_none());
    }

    #[test]
    fn t15_relative_reset_has_no_observed_request_but_can_have_mean() {
        let mut w = window(0.3, "2026-10-17T00:00:00Z"); w.reset_basis = Some(QuotaResetBasis::Relative);
        let i = run(w, "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(), None, QuotaHistoryState::Complete);
        assert_eq!(i.projection.unwrap().basis, Some(ProjectionBasis::CycleMean));
    }

    #[test]
    fn t16_session_has_quota_reset_display_and_no_forecast() {
        let mut w = window(0.9, "2026-10-14T04:00:00Z"); w.duration_seconds = Some(5 * 3600);
        let i = run(w, "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(), None, QuotaHistoryState::Complete);
        assert!(i.projection.is_none()); assert_eq!(i.quota_role, Some(QuotaRole::Session));
        let d = i.display_budget.unwrap(); assert_eq!(d.mode, DisplayBudgetMode::SessionQuotaReset); near(d.amount.unwrap(), 10.0);
    }

    #[test]
    fn t17_short_horizons_show_balance_without_daily_overstatement() {
        for (now, expected) in [("2026-10-16T00:00:00Z", DisplayBudgetMode::PerWorkday),
            ("2026-10-16T00:00:00.001Z", DisplayBudgetMode::UntilReset),
            ("2026-10-17T00:00:00Z", DisplayBudgetMode::ExtraDaysUntilReset)] {
            let i = run(window(0.1, "2026-10-18T00:00:00Z"), now, now, &calendar(), None, QuotaHistoryState::Complete);
            let d = i.display_budget.unwrap(); assert_eq!(d.mode, expected); near(d.amount.unwrap(), 90.0);
            if expected == DisplayBudgetMode::ExtraDaysUntilReset { assert_eq!(i.projection.unwrap().state, ProjectionState::NoRemainingWorkTime); }
        }
    }

    #[test]
    fn t20_stale_reference_uses_f_and_never_revalidates_current_budget() {
        let i = run(window(0.3, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:16:00Z", &calendar(), None, QuotaHistoryState::Complete);
        assert!(!i.fresh && i.budget.is_none()); assert_eq!(i.display_budget.unwrap().mode, DisplayBudgetMode::Unavailable);
        let r = i.last_reading_reference.unwrap(); assert_eq!(r.age_ms, 16 * 60 * 1000);
        near(r.per_remaining_workday_as_of.unwrap(), 70.0 / 3.0); assert!(!r.display_budget.unwrap().is_current);
        assert!(i.projection.unwrap().rate_per_workday.is_none());
    }

    #[test]
    fn t21_projected_due_preserves_official_balance_and_expired_reference_has_no_daily() {
        let i = run(window(0.9999, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:10:00Z", &calendar(),
            Some(vec![("2026-10-13T00:00:00Z", 70.0), ("2026-10-14T00:00:00Z", 99.99)]), QuotaHistoryState::Complete);
        assert_eq!(i.projection.unwrap().state, ProjectionState::ProjectedDue); near(i.official_remaining_percent.unwrap(), 0.01);
        let old = run(window(0.3, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-18T00:00:00Z", &calendar(), None, QuotaHistoryState::Complete);
        let r = old.last_reading_reference.unwrap(); assert!(r.previous_cycle && r.per_remaining_workday_as_of.is_none() && r.display_budget.is_none());
    }

    #[test]
    fn t22_same_calendar_manual_weekday_and_weekend_extra_rules() {
        let mut c = calendar(); c.holidays.push(Holiday { date: "2026-10-12".into(), name: "H".into(), source: "fixture".into() });
        c.overrides.push(CalendarOverride { date: "2026-10-12".into(), name: "W".into(), nonworking: false });
        c.overrides.push(CalendarOverride { date: "2026-10-11".into(), name: "W".into(), nonworking: false });
        let work = WorkingCalendar::new(&c).unwrap(); near(work.days(epoch("2026-10-11T00:00:00Z"), epoch("2026-10-13T00:00:00Z")).unwrap(), 1.0);
        c.covered_years.clear();
        let i = run(window(0.3, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &c, None, QuotaHistoryState::Complete);
        assert_eq!(i.projection.unwrap().reasons, vec![ProjectionReason::CalendarOutsideCoverage]);
    }

    #[test]
    fn t24_incomplete_truncated_unavailable_never_hide_under_mean() {
        for (state, expected) in [(QuotaHistoryState::Incomplete, ProjectionState::HistoryIncomplete),
            (QuotaHistoryState::Truncated, ProjectionState::HistoryTruncated), (QuotaHistoryState::Unavailable, ProjectionState::Unavailable)] {
            let p = run(window(0.3, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(),
                Some(vec![("2026-10-13T00:00:00Z", 10.0), ("2026-10-14T00:00:00Z", 30.0)]), state).projection.unwrap();
            assert_eq!(p.state, expected); assert!(p.rate_per_workday.is_none() && p.basis.is_none());
        }
        let p = run(window(0.3, "2026-10-17T00:00:00Z"), "2026-10-14T00:00:00Z", "2026-10-14T00:00:00Z", &calendar(), None, QuotaHistoryState::Complete).projection.unwrap();
        assert_eq!(p.state, ProjectionState::HistoryIncomplete);
    }

    #[test]
    fn t28_unknowns_are_null_and_private_history_is_not_serialized() {
        let p = empty_projection(BudgetUnit::Credits); let v = serde_json::to_value(p).unwrap();
        for field in ["basis", "sampled_from_ms", "rate_per_workday", "exhaustion_at_ms", "remaining_workdays"] { assert!(v[field].is_null()); }
        assert!(!v.as_object().unwrap().contains_key("series_key"));
        assert_eq!(classify_window("claude", "unknown_weekly_offer", Some(604800), None), (QuotaRole::Other, PeriodKind::Unknown));
        assert_eq!(classify_window("codex", "spark", Some(604800), None), (QuotaRole::Scoped, PeriodKind::Weekly));
    }

    #[test]
    fn t36_claude_without_identity_and_kiro_explicit_plan_can_use_cycle_mean() {
        for provider in ["claude", "kiro"] {
            let f = epoch("2026-10-14T00:00:00Z");
            let mut w = window(0.3, "2026-10-17T00:00:00Z"); w.quota_account_key = None;
            if provider == "claude" { w.id = "weekly_all".into(); w.duration_seconds = None; }
            else { w.id = "credits".into(); w.resets_at = Some(epoch("2026-11-01T00:00:00Z")); w.credit_scope = Some(CreditScope::Plan);
                w.credits = Some(CreditAmounts { consumed: 40.0, allowance: 1000.0 }); }
            let s = UsageSnapshot { status: "ok".into(), fetched_at: f, windows: vec![w], ..Default::default() };
            let c = calendar(); let mut b = compute_provider_insights(provider, &s, Some(&c), f);
            attach_projections(provider, &s, &mut b, Some(&c), &HashMap::new(), f);
            let p = b.windows[0].projection.as_ref().unwrap(); assert_eq!(p.basis, Some(ProjectionBasis::CycleMean));
            if provider == "kiro" { assert_eq!(p.unit, BudgetUnit::Credits); near(p.rate_per_workday.unwrap(), 40.0 / 9.0); }
        }
    }

    #[test]
    fn explicit_plan_credits_have_backend_percent_for_daily_balance_and_mean_rate() {
        let d = display(BudgetUnit::Credits, Some(1200.0), Some(4000.0), Some(1), Some(2), Some(6.0), false, true);
        assert_eq!(d.mode, DisplayBudgetMode::PerWorkday);
        near(d.amount.unwrap(), 200.0); near(d.amount_percent.unwrap(), 5.0);
        let until_reset = display(BudgetUnit::Credits, Some(1200.0), Some(4000.0), Some(1), Some(2), Some(0.5), false, true);
        assert_eq!(until_reset.mode, DisplayBudgetMode::UntilReset);
        near(until_reset.amount.unwrap(), 1200.0); near(until_reset.amount_percent.unwrap(), 30.0);

        let f = epoch("2026-10-14T08:00:00Z"); let c = calendar();
        let w = LimitWindow { id: "credits".into(), used: 0.7, resets_at: Some(epoch("2026-11-01T00:00:00Z")),
            credit_scope: Some(CreditScope::Plan), credits: Some(CreditAmounts { consumed: 2800.0, allowance: 4000.0 }), ..Default::default() };
        let s = UsageSnapshot { status: "ok".into(), fetched_at: f, windows: vec![w], ..Default::default() };
        let mut b = compute_provider_insights("kiro", &s, Some(&c), f);
        attach_projections("kiro", &s, &mut b, Some(&c), &HashMap::new(), f);
        let p = b.windows[0].projection.as_ref().unwrap();
        assert_eq!(p.basis, Some(ProjectionBasis::CycleMean)); near(p.rate_per_workday.unwrap(), 300.0); near(p.rate_percent.unwrap(), 7.5);

        let now = f + 16 * 60 * 1000;
        let mut stale = compute_provider_insights("kiro", &s, Some(&c), now);
        attach_projections("kiro", &s, &mut stale, Some(&c), &HashMap::new(), now);
        let old = stale.windows[0].last_reading_reference.as_ref().unwrap().display_budget.as_ref().unwrap();
        assert!(!old.is_current); assert_eq!(old.as_of_ms, Some(f));
        near(old.amount_percent.unwrap(), old.amount.unwrap() / 4000.0 * 100.0);
        assert!(stale.windows[0].projection.as_ref().unwrap().rate_percent.is_none());
    }

    #[test]
    fn percent_fields_preserve_unknowns_and_legacy_deserialization() {
        for allowance in [None, Some(0.0), Some(-1.0), Some(f64::NAN), Some(f64::INFINITY)] {
            assert!(percent_of_allowance(BudgetUnit::Credits, Some(200.0), allowance).is_none());
        }
        assert!(percent_of_allowance(BudgetUnit::Credits, None, Some(4000.0)).is_none());
        assert!(percent_of_allowance(BudgetUnit::Credits, Some(f64::INFINITY), Some(4000.0)).is_none());
        let d = display(BudgetUnit::Credits, Some(1200.0), Some(4000.0), Some(1), Some(2), Some(6.0), false, false);
        assert!(d.amount.is_none() && d.amount_percent.is_none());
        let mut legacy = serde_json::to_value(d).unwrap(); legacy.as_object_mut().unwrap().remove("amount_percent");
        assert!(serde_json::from_value::<DisplayBudget>(legacy).unwrap().amount_percent.is_none());
        let mut p = empty_projection(BudgetUnit::Credits); p.rate_per_workday = Some(300.0); p.rate_percent = Some(7.5);
        fail(&mut p, ProjectionState::Unavailable, ProjectionReason::InvalidReading);
        assert!(p.rate_percent.is_none() && p.rate_per_workday.is_none());
        let mut legacy = serde_json::to_value(p).unwrap(); legacy.as_object_mut().unwrap().remove("rate_percent");
        assert!(serde_json::from_value::<QuotaProjection>(legacy).unwrap().rate_percent.is_none());
    }
}
