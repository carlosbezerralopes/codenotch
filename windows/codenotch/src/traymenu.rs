//! The words the tray menu prints, and nothing else: no menu, no Tauri, so every line can be
//! tested on its own.
//!
//! The page has these strings too (`TEXT` in notch.html), and they are kept here under the exact
//! same English keys, the way `i18n.rs` already mirrors the page's dictionary. `the_card_and_the_menu_agree`
//! below fails if the two ever drift apart.
//!
//! Dates and times come from Windows rather than a table of month names: the menu should read the
//! way the taskbar clock does, including whether the user's region writes 4:59 AM or 16:59.

use crate::usage::{LimitWindow, UsageSnapshot};

pub fn matching_insight<'a>(dto: &'a crate::usage_insights::ProviderInsights, source: &LimitWindow, index: usize) -> Option<&'a crate::usage_insights::WindowInsights> {
    dto.windows.iter().find(|w| w.window_index == index && w.window_id == source.id && w.group == source.group)
}

pub fn insight_target(id: &str) -> Option<(&str, usize)> {
    let mut parts = id.split(':');
    if parts.next()? != "insight" { return None; }
    let provider = parts.next()?;
    if !matches!(provider, "claude" | "codex" | "kiro") { return None; }
    let raw = parts.next()?;
    if raw.is_empty() || raw.len() > 4 || !raw.bytes().all(|b| b.is_ascii_digit()) || parts.next().is_some() { return None; }
    let index = raw.parse::<usize>().ok()?;
    (index <= 4095).then_some((provider, index))
}

/// Windows counts UTF-16 code units, not Rust bytes or Unicode scalar values.
pub fn bounded_tooltip(text: &str) -> String {
    if text.encode_utf16().count() <= 128 { return text.into(); }
    let mut result = String::new();
    let mut units = 0;
    for ch in text.chars() {
        if units + ch.len_utf16() > 127 { break; }
        result.push(ch);
        units += ch.len_utf16();
    }
    result.push('…');
    result
}

fn legacy_insight_lines(w: &crate::usage_insights::WindowInsights, lang: &str) -> Vec<String> {
    use crate::usage_insights::{BudgetUnit, InsightReason};
    let pt = lang == "pt-BR";
    let num = |v: f64| { let s = format!("{v:.2}"); if pt { s.replace('.', ",") } else { s } };
    let mut lines = Vec::new();
    if let Some(c) = &w.credits {
        lines.push(format!("{}: {} {}", if pt { "Saldo" } else { "Remaining" }, num(c.remaining), if pt { "créditos" } else { "credits" }));
    }
    if !w.fresh {
        let stale = w.reasons.contains(&InsightReason::Stale);
        lines.push(if stale {
            if pt { "Leitura desatualizada; referências indisponíveis." } else { "Outdated reading; references unavailable." }
        } else if pt { "Referências derivadas indisponíveis." } else { "Derived references unavailable." }.into());
    } else {
        if let Some(work) = &w.work {
            let until = w.period.as_ref().map(|p| system_datetime(p.reset_ms)).unwrap_or_default();
            lines.push(format!("{} {} {} {}", num(work.remaining_days), if pt { "dias úteis até" } else { "business days until" }, if pt { "renovação" } else { "reset" }, until));
            if let Some(gap) = work.used_minus_work_pp {
                lines.push(format!("{} pp {}", num(gap.abs()), if pt { if gap >= 0.0 { "acima do tempo útil no ciclo" } else { "abaixo do tempo útil no ciclo" } } else if gap >= 0.0 { "above business time in the cycle" } else { "below business time in the cycle" }));
            }
        }
        if let Some(b) = &w.budget {
            let unit = match b.unit { BudgetUnit::Credits => if pt { " créditos" } else { " credits" }, BudgetUnit::Percent => "%" };
            lines.push(format!("{}: {}{} · {}: {}{}", if pt { "Disponível/dia útil restante" } else { "Available/remaining business day" }, num(b.per_remaining_workday), unit, if pt { "Nesta semana até renovação" } else { "This week until reset" }, num(b.week_remaining_budget), unit));
            if let Some(reference) = b.reference_per_cycle_workday {
                lines.push(format!("{}: {}{}", if pt { "Referência do ciclo/dia útil" } else { "Cycle reference/business day" }, num(reference), unit));
            }
            if let Some(reference) = b.week_reference_budget {
                lines.push(format!("{}: {}{}", if pt { "Referência do ciclo nesta semana" } else { "Cycle reference this week" }, num(reference), unit));
            }
            lines.push(if pt { "Referência derivada; o provedor define a cota oficial." } else { "Derived reference; the provider defines the official quota." }.into());
        }
    }
    for reason in &w.reasons {
        let text = match reason {
            InsightReason::UnknownCreditScope => Some(if pt { "Saldo pode incluir adicionais; referência mensal indisponível." } else { "Balance may include add-ons; monthly reference unavailable." }),
            InsightReason::NonrenewingWallet => Some(if pt { "Bônus/excedente separados da cota mensal." } else { "Bonus/overage are separate from the monthly quota." }),
            InsightReason::CalendarPartial => Some(if pt { "Calendário parcial; feriados locais não confirmados." } else { "Partial calendar; local holidays unconfirmed." }),
            InsightReason::CalendarMissing => Some(if pt { "Calendário indisponível." } else { "Calendar unavailable." }),
            InsightReason::CalendarOutsideCoverage => Some(if pt { "Calendário não cobre o intervalo." } else { "Calendar does not cover the interval." }),
            InsightReason::UnknownStart => Some(if pt { "Início do ciclo desconhecido." } else { "Cycle start unknown." }),
            InsightReason::NoRemainingWorkdays => Some(if pt { "Sem tempo útil restante." } else { "No business time remains." }),
            InsightReason::NoWorkdays => Some(if pt { "Sem tempo útil neste ciclo." } else { "No business time in this cycle." }),
            InsightReason::MissingReset => Some(if pt { "Data de renovação indisponível." } else { "Reset date unavailable." }),
            InsightReason::ResetExpired => Some(if pt { "Aguardando nova data de renovação." } else { "Waiting for a new reset date." }),
            InsightReason::InvalidPeriod => Some(if pt { "Intervalo do ciclo indisponível." } else { "Cycle interval unavailable." }),
            InsightReason::MissingReading | InsightReason::InvalidReading => Some(if pt { "Leitura da cota indisponível." } else { "Quota reading unavailable." }),
            InsightReason::MissingWindowFreshness => Some(if pt { "A atualização desta conta não foi confirmada." } else { "The freshness of this account reading is unknown." }),
            _ => None,
        };
        if let Some(text) = text { lines.push(text.into()); }
    }
    lines
}

pub fn insight_lines(w: &crate::usage_insights::WindowInsights, lang: &str) -> Vec<String> {
    use crate::usage_insights::{DisplayBudgetMode, ProjectionBasis, ProjectionReason, ProjectionState, QuotaRole};
    // Old cached DTOs retain their qualified references until the next reading.
    if w.quota_role.is_none() { return legacy_insight_lines(w, lang); }
    if w.quota_role != Some(QuotaRole::General) { return Vec::new(); }
    let pt = lang == "pt-BR";
    let mut lines = Vec::new();
    if !w.fresh {
        let text = w.last_reading_reference.as_ref().map(|r| format!("{} {}", if pt { "Última leitura:" } else { "Last reading:" }, system_datetime(r.as_of_ms)))
            .unwrap_or_else(|| if pt { "Leitura indisponível" } else { "Reading unavailable" }.into());
        lines.push(text);
        return lines;
    }
    if let Some(b) = &w.display_budget {
        if let Some(amount) = b.amount.filter(|_| b.is_current) {
            let caption = match b.mode {
                DisplayBudgetMode::PerWorkday => Some(if pt { "Pode usar/dia útil" } else { "Available/workday" }),
                DisplayBudgetMode::UntilReset | DisplayBudgetMode::ExtraDaysUntilReset => Some(if pt { "Saldo até renovar" } else { "Balance until reset" }),
                _ => None,
            };
            if let Some(caption) = caption { lines.push(format!("{caption}: {}", budget_amount(amount, b.amount_percent, b.unit, pt))); }
        }
    }
    if let Some(p) = &w.projection {
        if let Some(rate) = p.rate_per_workday {
            let caption = if p.basis == Some(ProjectionBasis::ObservedRecent) {
                if pt { "Ritmo observado/dia útil" } else { "Observed pace/workday" }
            } else { if pt { "Média do ciclo/dia útil" } else { "Cycle mean/workday" } };
            lines.push(format!("{caption}: {}", budget_amount(rate, p.rate_percent, p.unit, pt)));
        }
        let early = p.early_workdays.map(|d| if d > 0.0 && d < 0.1 { if pt { "<0,1" } else { "<0.1" }.into() } else { format!("{d:.1}").replace('.', if pt { "," } else { "." }) }).unwrap_or_else(|| "—".into());
        let unavailable = if p.reasons.iter().any(|r| matches!(r, ProjectionReason::CalendarMissing | ProjectionReason::CalendarOutsideCoverage)) {
            if pt { "Previsão indisponível; revisar calendário" } else { "Forecast unavailable; check calendar" }
        } else if p.reasons.iter().any(|r| matches!(r, ProjectionReason::MissingReset | ProjectionReason::UnknownStart | ProjectionReason::InvalidPeriod | ProjectionReason::ResetExpired)) {
            if pt { "Previsão indisponível; período não confirmado" } else { "Forecast unavailable; period not confirmed" }
        } else if p.reasons.contains(&ProjectionReason::HistoryUnavailable) {
            if pt { "Histórico indisponível para previsão" } else { "History unavailable for forecasting" }
        } else { if pt { "Previsão aguardando histórico suficiente" } else { "Forecast awaiting sufficient history" } };
        lines.push(match p.state {
            ProjectionState::BeforeReset => format!("{} {early} {}", if pt { "Neste ritmo, esgota" } else { "At this pace, runs out" }, if pt { "dias úteis antes de renovar" } else { "workdays before reset" }),
            ProjectionState::ProjectedDue => if pt { "Esgotamento projetado já alcançado; conferir saldo oficial" } else { "Projected exhaustion reached; check official balance" }.into(),
            ProjectionState::CoversReset => if pt { "Saldo cobre até a renovação neste ritmo" } else { "Balance covers reset at this pace" }.into(),
            ProjectionState::Exhausted => if pt { "Cota oficial esgotada" } else { "Official quota exhausted" }.into(),
            ProjectionState::InsufficientWorkTime => if pt { "Ainda sem tempo útil suficiente para medir o ritmo" } else { "Not enough business time to measure pace yet" }.into(),
            ProjectionState::NoRemainingWorkTime => if pt { "Restam apenas dias extras até renovar" } else { "Only extra days remain before reset" }.into(),
            _ => unavailable.into(),
        });
    }
    lines
}

fn budget_amount(amount: f64, percent: Option<f64>, unit: crate::usage_insights::BudgetUnit, pt: bool) -> String {
    let num = |n: f64| format!("{n:.1}").replace('.', if pt { "," } else { "." });
    match unit {
        crate::usage_insights::BudgetUnit::Percent => format!("{}%", num(amount)),
        crate::usage_insights::BudgetUnit::Credits => format!("{} cr{}", num(amount), percent.map(|p| format!(" ({}%)", num(p))).unwrap_or_default()),
    }
}

pub fn insight_tooltip(w: &crate::usage_insights::WindowInsights, lang: &str) -> Option<String> {
    use crate::usage_insights::DisplayBudgetMode;
    let b = w.display_budget.as_ref()?;
    if !w.fresh || !b.is_current || b.mode != DisplayBudgetMode::PerWorkday { return None; }
    Some(format!("{}/{}", budget_amount(b.amount?, b.amount_percent, b.unit, lang == "pt-BR"), if lang == "pt-BR" { "útil" } else { "workday" }))
}

/// A reading as the card prints it: whole percents, except where rounding would claim nothing is
/// used or nothing is left.
pub fn pct(fraction: f64) -> String {
    let v = (fraction * 100.0).clamp(0.0, 100.0);
    if v <= 0.0 {
        return "0".into();
    }
    // Tenths only where rounding would lie: a sliver used reading as 0, or a sliver left as 100.
    // Exactly 100 is not a sliver, so it stays a whole number.
    if v < 1.0 || (v > 99.0 && v < 100.0) {
        let tenth = (v * 10.0).round() / 10.0;
        if tenth < 0.1 {
            "<0.1".into()
        } else if tenth > 99.9 {
            ">99.9".into()
        } else {
            format!("{tenth:.1}")
        }
    } else {
        format!("{}", v.round() as u32)
    }
}

/// "61% Used · 39% left". Both ends, because vendors disagree about which one they publish, and the
/// left half is taken from the rounded used half so the two always add up on screen.
pub fn used_left(w: &LimitWindow, lang: &str) -> String {
    let v = (w.used * 100.0).clamp(0.0, 100.0);
    let (used, left) = if (v > 0.0 && v < 1.0) || (v > 99.0 && v < 100.0) {
        (pct(w.used), pct((100.0 - v) / 100.0))
    } else {
        let u = v.round() as u32;
        (u.to_string(), (100 - u.min(100)).to_string())
    };
    let used = if w.derived { format!("~{used}") } else { used };
    match lang {
        "pt-BR" => format!("{used}% usado · {left}% restante"),
        "ru" => format!("Использовано {used}% · осталось {left}%"),
        "zh" => format!("已用 {used}% · 剩余 {left}%"),
        "zh-Hant" => format!("已用 {used}% · 剩餘 {left}%"),
        "ja" => format!("{used}% 使用 · 残り {left}%"),
        "uk" => format!("Використано {used}% · Лишилось {left}%"),
        "ko" => format!("{used}% 사용 · {left}% 남음"),
        _ => format!("{used}% Used · {left}% left"),
    }
}

/// "Resets in 59 min", "Resets in 2h 15m", "Resets in 3 Days 4h", or the date and time once it is
/// far enough out that a duration stops meaning anything.
pub fn reset_text(resets_at: u64, now: u64, lang: &str) -> String {
    if resets_at <= now {
        return match lang {
            "pt-BR" => "Renovando…",
            "ru" => "Сброс…",
            "zh" => "正在重置…",
            "zh-Hant" => "正在重置…",
            "ja" => "リセット中…",
            "uk" => "Скидання…",
            "ko" => "재설정 중…",
            _ => "Resetting…",
        }
        .into();
    }
    let minutes = ((resets_at - now) as f64 / 60_000.0).round() as u64;
    let (hours, days) = (minutes / 60, minutes / 1440);
    if minutes < 60 {
        let m = minutes.max(1);
        return match lang {
            "pt-BR" => format!("Renova em {m} min"),
            "ru" => format!("Сброс через {m} мин"),
            "zh" => format!("{m} 分钟后重置"),
            "zh-Hant" => format!("{m} 分鐘後重置"),
            "ja" => format!("{m} 分後にリセット"),
            "uk" => format!("Скидання через {m} хв"),
            "ko" => format!("{m}분 후 재설정"),
            _ => format!("Resets in {m} min"),
        };
    }
    if hours < 24 {
        let (h, m) = (hours, minutes % 60);
        return match lang {
            "pt-BR" => format!("Renova em {h}h {m}m"),
            "ru" => format!("Сброс через {h} ч {m} мин"),
            "zh" => format!("{h} 小时 {m} 分钟后重置"),
            "zh-Hant" => format!("{h} 小時 {m} 分鐘後重置"),
            "ja" => format!("{h} 時間 {m} 分後にリセット"),
            "uk" => format!("Скидання через {h} год {m} хв"),
            "ko" => format!("{h}시간 {m}분 후 재설정"),
            _ => format!("Resets in {h}h {m}m"),
        };
    }
    if days < 7 {
        let (d, h) = (days, hours % 24);
        return match lang {
            "pt-BR" => if d == 1 { format!("Renova em {d} dia {h}h") } else { format!("Renova em {d} dias {h}h") },
            "ru" => format!("Сброс через {d} дн. {h} ч"),
            "zh" => format!("{d} 天 {h} 小时后重置"),
            "zh-Hant" => format!("{d} 天 {h} 小時後重置"),
            "ja" => format!("{d} 日 {h} 時間後にリセット"),
            "uk" => format!("Скидання через {d} дн {h} год"),
            "ko" => format!("{d}일 {h}시간 후 재설정"),
            _ if d == 1 => format!("Resets in {d} Day {h}h"),
            _ => format!("Resets in {d} Days {h}h"),
        };
    }
    let when = system_datetime(resets_at);
    match lang {
        "pt-BR" => format!("Renova {when}"),
        "ru" => format!("Сброс: {when}"),
        "zh" => format!("{when} 重置"),
        "zh-Hant" => format!("{when} 重置"),
        "ja" => format!("{when} にリセット"),
        "uk" => format!("Скидання {when}"),
        "ko" => format!("{when}에 재설정"),
        _ => format!("Resets {when}"),
    }
}

/// "20 min ago", for a reading that has gone stale.
pub fn ago(since: u64, now: u64, lang: &str) -> String {
    let minutes = now.saturating_sub(since) / 60_000;
    let span = if minutes < 60 {
        match lang {
            "pt-BR" => format!("{minutes} min"),
            "ru" => format!("{minutes} мин"),
            "zh" => format!("{minutes} 分钟"),
            "zh-Hant" => format!("{minutes} 分鐘"),
            "ja" => format!("{minutes} 分"),
            "uk" => format!("{minutes} хв"),
            "ko" => format!("{minutes}분"),
            _ => format!("{minutes}m"),
        }
    } else {
        let h = (minutes as f64 / 60.0).round() as u64;
        match lang {
            "pt-BR" => format!("{h} h"),
            "ru" => format!("{h} ч"),
            "zh" => format!("{h} 小时"),
            "zh-Hant" => format!("{h} 小時"),
            "ja" => format!("{h} 時間"),
            "uk" => format!("{h} год"),
            "ko" => format!("{h}시간"),
            _ => format!("{h}h"),
        }
    };
    match lang {
        "pt-BR" => format!("há {span}"),
        "ru" => format!("{span} назад"),
        "zh" => format!("{span}前"),
        "zh-Hant" => format!("{span}前"),
        "ja" => format!("{span}前"),
        "uk" => format!("{span} тому"),
        "ko" => format!("{span} 전"),
        _ => format!("{span} ago"),
    }
}

/// The window names the providers publish. English is the key on both sides, so a provider that
/// starts sending a name nobody has translated prints that name rather than nothing.
pub fn label(name: &str, lang: &str) -> String {
    let translated = match (lang, name) {
        ("pt-BR", "Current session") => "Sessão atual",
        ("pt-BR", "Weekly (all models)") => "Semanal (todos os modelos)",
        ("pt-BR", "Weekly (Opus)") => "Semanal (Opus)",
        ("pt-BR", "Weekly (model-scoped)") => "Semanal (por modelo)",
        ("pt-BR", "Weekly limit" | "Weekly Limit") => "Limite semanal",
        ("pt-BR", "Monthly limit" | "Monthly Limit") => "Limite mensal",
        ("pt-BR", "5-hour Limit" | "5-Hour Limit") => "Limite de 5 horas",
        ("pt-BR", "Included usage") => "Uso incluído",
        ("pt-BR", "API usage") => "Uso da API",
        ("pt-BR", "Credits") => "Créditos",
        ("pt-BR", "Bonus") => "Bônus",
        ("pt-BR", "Overage") => "Uso excedente",
        ("pt-BR", "Premium requests") => "Solicitações premium",
        ("pt-BR", "Chat requests") => "Solicitações de chat",
        ("pt-BR", "Completions") => "Conclusões de código",
        ("ru", "Current session") => "Текущий сеанс",
        ("ru", "Weekly (all models)") => "Недельный (все модели)",
        ("ru", "Weekly (Opus)") => "Недельный (Opus)",
        ("ru", "Weekly (model-scoped)") => "Недельный (для выбранной модели)",
        ("ru", "Weekly limit" | "Weekly Limit") => "Недельный лимит",
        ("ru", "Monthly limit" | "Monthly Limit") => "Месячный лимит",
        ("ru", "5-hour Limit" | "5-Hour Limit") => "Лимит на 5 часов",
        ("ru", "Included usage") => "Включённое использование",
        ("ru", "API usage") => "Использование API",
        ("ru", "Premium requests") => "Премиум-запросы",
        ("ru", "Chat requests") => "Запросы чата",
        ("ru", "Completions") => "Автодополнения",
        ("zh", "Current session") => "当前会话",
        ("zh", "Weekly (all models)") => "每周（全部模型）",
        ("zh", "Weekly (Opus)") => "每周（Opus）",
        ("zh", "Weekly (model-scoped)") => "每周（指定模型）",
        ("zh", "Weekly limit" | "Weekly Limit") => "每周限额",
        ("zh", "Monthly limit" | "Monthly Limit") => "每月限额",
        ("zh", "5-hour Limit" | "5-Hour Limit") => "5 小时限额",
        ("zh", "Included usage") => "包含用量",
        ("zh", "API usage") => "API 用量",
        ("zh", "Premium requests") => "高级请求",
        ("zh", "Chat requests") => "聊天请求",
        ("zh", "Completions") => "代码补全",
        ("zh-Hant", "Current session") => "目前工作階段",
        ("zh-Hant", "Weekly (all models)") => "每週（全部模型）",
        ("zh-Hant", "Weekly (Opus)") => "每週（Opus）",
        ("zh-Hant", "Weekly (model-scoped)") => "每週（指定模型）",
        ("zh-Hant", "Weekly limit" | "Weekly Limit") => "每週限額",
        ("zh-Hant", "Monthly limit" | "Monthly Limit") => "每月限額",
        ("zh-Hant", "5-hour Limit" | "5-Hour Limit") => "5 小時限額",
        ("zh-Hant", "Included usage") => "包含用量",
        ("zh-Hant", "API usage") => "API 用量",
        ("zh-Hant", "Premium requests") => "進階請求",
        ("zh-Hant", "Chat requests") => "聊天請求",
        ("zh-Hant", "Completions") => "程式碼補全",
        ("ja", "Current session") => "現在のセッション",
        ("ja", "Weekly (all models)") => "週間 (すべてのモデル)",
        ("ja", "Weekly (Opus)") => "週間 (Opus)",
        ("ja", "Weekly (model-scoped)") => "週間 (モデル別)",
        ("ja", "Weekly limit" | "Weekly Limit") => "週間の上限",
        ("ja", "Monthly limit" | "Monthly Limit") => "月間の上限",
        ("ja", "5-hour Limit" | "5-Hour Limit") => "5 時間の上限",
        ("ja", "Included usage") => "プラン内の使用量",
        ("ja", "API usage") => "API 使用量",
        ("ja", "Premium requests") => "プレミアムリクエスト",
        ("ja", "Chat requests") => "チャットリクエスト",
        ("ja", "Completions") => "コード補完",
        ("uk", "Current session") => "Поточна сесія",
        ("uk", "Weekly (all models)") => "Тижневий (усі моделі)",
        ("uk", "Weekly (Opus)") => "Тижневий (Opus)",
        ("uk", "Weekly (model-scoped)") => "Тижневий (за моделями)",
        ("uk", "Weekly limit" | "Weekly Limit") => "Тижневий ліміт",
        ("uk", "Monthly limit" | "Monthly Limit") => "Місячний ліміт",
        ("uk", "5-hour Limit" | "5-Hour Limit") => "Ліміт 5 годин",
        ("uk", "Included usage") => "Використання в тарифі",
        ("uk", "API usage") => "Використання API",
        ("uk", "Premium requests") => "Преміум-запити",
        ("uk", "Chat requests") => "Запити чату",
        ("uk", "Completions") => "Автодоповнення",
        ("ko", "Current session") => "현재 세션",
        ("ko", "Included usage") => "포함 사용량",
        ("ko", "API usage") => "API 사용량",
        ("ko", "Weekly limit" | "Weekly Limit") => "주간 제한",
        ("ko", "Monthly limit" | "Monthly Limit") => "월간 제한",
        ("ko", "5-hour Limit" | "5-Hour Limit") => "5시간 제한",
        ("ko", "Weekly (all models)") => "주간 (모든 모델)",
        ("ko", "Weekly (Opus)") => "주간 (Opus)",
        ("ko", "Weekly (model-scoped)") => "주간 (모델별)",
        ("ko", "Premium requests") => "프리미엄 요청",
        ("ko", "Chat requests") => "채팅 요청",
        ("ko", "Completions") => "코드 완성",
        _ => name,
    };
    translated.into()
}

/// "Claude — 61%", plus how old the reading is once it has gone stale.
///
/// The fraction rather than a rounded percentage: Antigravity publishes lanes like 0.5 %, and a
/// header reading 1 % above a line reading 0.5 % is the app contradicting itself.
pub fn header(provider: &str, reading: Option<f64>, stale_since: Option<u64>, now: u64, lang: &str) -> String {
    let value = reading.map(|f| format!("{}%", pct(f))).unwrap_or_else(|| "—".into());
    // Under a minute is not worth saying. A provider that re-reads while still flagged stale would
    // otherwise head every line with "0m ago", which reads as a fault rather than as an age.
    match stale_since.filter(|since| now.saturating_sub(*since) >= 60_000) {
        Some(since) => format!("{provider} — {value} · {}", ago(since, now, lang)),
        None => format!("{provider} — {value}"),
    }
}

/// "Current session: 61% Used · 39% left · Resets in 59 min". A window with no denominator says how
/// many requests there were instead, because a percentage of an unpublished limit is a guess.
pub fn window_line(w: &LimitWindow, now: u64, lang: &str) -> String {
    let name = label(&w.label, lang);
    if let Some(count) = w.count {
        return format!("{name}: ~{count}");
    }
    let mut line = format!("{name}: {}", used_left(w, lang));
    if let Some(at) = w.resets_at {
        line.push_str(" · ");
        line.push_str(&reset_text(at, now, lang));
    }
    line
}

/// Every line one provider contributes: its header, then a line per window it publishes.
pub fn provider_lines(snap: &UsageSnapshot, now: u64, lang: &str) -> Vec<String> {
    if snap.windows.is_empty() {
        return match snap.note.is_empty() {
            true => Vec::new(),
            false => vec![snap.note.clone()],
        };
    }
    snap.windows.iter().map(|w| window_line(w, now, lang)).collect()
}

/// When the reading is old enough to say so: the page draws the same cell dimmed on the same rule.
/// Fifteen minutes, matching `staleOf` in notch.html and the Mac's `UsageStore.staleAfter`.
pub fn stale_since(snap: &UsageSnapshot, now: u64) -> Option<u64> {
    let old = snap.fetched_at > 0 && now.saturating_sub(snap.fetched_at) > 15 * 60 * 1000;
    (snap.status == "stale" || old).then_some(snap.fetched_at).filter(|t| *t > 0)
}

/// The date and time as this machine writes them, so "Resets Thu, 4:59 AM" follows the same region
/// settings as the clock in the corner.
#[cfg(windows)]
fn system_datetime(ms: u64) -> String {
    use windows::core::PCWSTR;
    use windows::Win32::Globalization::{
        GetDateFormatEx, GetTimeFormatEx, DATE_SHORTDATE, TIME_NOSECONDS,
    };
    let Some(st) = local_systemtime(ms) else { return String::new() };
    let mut date = [0u16; 80];
    let mut time = [0u16; 80];
    let (d, t) = unsafe {
        (
            GetDateFormatEx(PCWSTR::null(), DATE_SHORTDATE, Some(&st), PCWSTR::null(), Some(&mut date), PCWSTR::null()),
            GetTimeFormatEx(PCWSTR::null(), TIME_NOSECONDS, Some(&st), PCWSTR::null(), Some(&mut time)),
        )
    };
    if d <= 0 || t <= 0 {
        return String::new();
    }
    let text = |buf: &[u16], n: i32| String::from_utf16_lossy(&buf[..(n as usize - 1)]);
    format!("{} {}", text(&date, d), text(&time, t))
}

/// Epoch milliseconds as local wall-clock time. chrono owns the calendar arithmetic; Windows only
/// formats what it is given.
#[cfg(windows)]
fn local_systemtime(ms: u64) -> Option<windows::Win32::Foundation::SYSTEMTIME> {
    use chrono::{Datelike, Local, TimeZone, Timelike};
    let local = Local.timestamp_millis_opt(ms as i64).single()?;
    Some(windows::Win32::Foundation::SYSTEMTIME {
        wYear: local.year() as u16,
        wMonth: local.month() as u16,
        wDayOfWeek: local.weekday().num_days_from_sunday() as u16,
        wDay: local.day() as u16,
        wHour: local.hour() as u16,
        wMinute: local.minute() as u16,
        wSecond: 0,
        wMilliseconds: 0,
    })
}

#[cfg(not(windows))]
fn system_datetime(_ms: u64) -> String {
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(label: &str, used: f64, resets_at: Option<u64>) -> LimitWindow {
        LimitWindow { id: "w".into(), label: label.into(), used, resets_at, ..Default::default() }
    }

    const MIN: u64 = 60 * 1000;

    #[test]
    fn insight_links_are_bounded_and_have_an_exact_provider() {
        assert_eq!(insight_target("insight:claude:0"), Some(("claude", 0)));
        assert_eq!(insight_target("insight:kiro:4095"), Some(("kiro", 4095)));
        for raw in ["insight:codex:4096", "insight:codex:-1", "insight:codex:1:2", "insight:evil:0", "insight:claude:1e3", "insight:kiro:", "refresh:claude"] {
            assert_eq!(insight_target(raw), None, "{raw}");
        }
    }

    #[test]
    fn tray_insight_attachment_rejects_reordered_or_replaced_account_windows() {
        let source = LimitWindow { id: "weekly@a".into(), group: Some("Account A".into()), ..Default::default() };
        let snapshot = UsageSnapshot { windows: vec![source.clone()], ..Default::default() };
        let dto = crate::usage_insights::compute_provider_insights("claude", &snapshot, None, 1000);
        assert!(matching_insight(&dto, &source, 0).is_some());
        assert!(matching_insight(&dto, &source, 1).is_none());
        let changed_group = LimitWindow { group: Some("Account B".into()), ..source.clone() };
        assert!(matching_insight(&dto, &changed_group, 0).is_none());
        let changed_id = LimitWindow { id: "weekly@b".into(), ..source.clone() };
        assert!(matching_insight(&dto, &changed_id, 0).is_none());
    }

    #[test]
    fn tray_hover_respects_windows_utf16_limit_without_splitting_surrogates() {
        assert_eq!(bounded_tooltip("Codenotch — Claude 25%"), "Codenotch — Claude 25%");
        let text = format!("{}😀suffix", "a".repeat(126));
        let bounded = bounded_tooltip(&text);
        assert_eq!(bounded, format!("{}…", "a".repeat(126)));
        assert!(bounded.encode_utf16().count() <= 128);
        assert_eq!(bounded_tooltip(&"😀".repeat(70)).chars().last(), Some('…'));
    }

    #[test]
    fn tray_insights_format_dto_budget_and_explain_unknown_credit_scope() {
        let mut w: crate::usage_insights::WindowInsights = serde_json::from_value(serde_json::json!({
            "window_index": 3, "window_id": "credits", "label": "Credits", "group": null,
            "fresh": true, "official_used_percent": 25, "official_remaining_percent": 75,
            "credits": { "consumed": 82.79, "allowance": 5000, "remaining": 4917.21, "scope": "plan" },
            "period": null, "work": null,
            "budget": { "unit": "credits", "remaining": 4917.21, "per_remaining_workday": 245.86,
                "reference_per_cycle_workday": 250, "week_start_ms": 1, "week_end_ms": 2,
                "week_remaining_workdays": 2, "week_remaining_budget": 491.72, "week_reference_budget": 500 },
            "reasons": []
        })).unwrap();
        let pt = insight_lines(&w, "pt-BR").join("\n");
        assert!(pt.contains("4917,21 créditos"));
        assert!(pt.contains("245,86 créditos"));
        assert!(pt.contains("491,72 créditos"));
        assert!(pt.contains("Referência derivada"));
        assert!(!pt.contains("245,86%"));
        w.fresh = false;
        assert!(!insight_lines(&w, "en").join("\n").contains("245.86"));
        w.fresh = true; w.budget = None;
        w.reasons = vec![crate::usage_insights::InsightReason::UnknownCreditScope];
        let en = insight_lines(&w, "en").join("\n");
        assert!(en.contains("monthly reference unavailable"));
        assert!(!en.contains("Available/remaining"));
        w.fresh = false;
        w.reasons = vec![crate::usage_insights::InsightReason::MissingWindowFreshness];
        let unknown = insight_lines(&w, "pt-BR").join("\n");
        assert!(unknown.contains("A atualização desta conta não foi confirmada"));
        assert!(!unknown.contains("Leitura desatualizada"));
    }

    #[test]
    fn balance_and_deadline_stays_compact_and_uses_dto_units() {
        let source = LimitWindow { id: "credits".into(), label: "Credits".into(), used: 0.25, ..Default::default() };
        let snapshot = UsageSnapshot { windows: vec![source], ..Default::default() };
        let mut w = crate::usage_insights::compute_provider_insights("kiro", &snapshot, None, 1000).windows.remove(0);
        w.fresh = true;
        w.quota_role = Some(crate::usage_insights::QuotaRole::General);
        w.display_budget = Some(serde_json::from_value(serde_json::json!({
            "mode": "per_workday", "unit": "credits", "amount": 200, "amount_percent": 5,
            "workdays_remaining": 6, "as_of_ms": 1000, "is_current": true
        })).unwrap());
        assert_eq!(insight_lines(&w, "pt-BR"), vec!["Pode usar/dia útil: 200,0 cr (5,0%)"]);
        assert_eq!(insight_tooltip(&w, "pt-BR").unwrap(), "200,0 cr (5,0%)/útil");
        w.projection = Some(serde_json::from_value(serde_json::json!({
            "state": "unavailable", "unit": "credits", "sample_count": 0,
            "extra_or_mixed_consumption": false, "reasons": ["calendar_outside_coverage"]
        })).unwrap());
        assert!(insight_lines(&w, "pt-BR").join("\n").contains("revisar calendário"));
        assert!(!insight_lines(&w, "pt-BR").join("\n").contains("aguardando histórico"));
        w.projection.as_mut().unwrap().state = crate::usage_insights::ProjectionState::BeforeReset;
        w.projection.as_mut().unwrap().early_workdays = Some(0.01);
        assert!(insight_lines(&w, "en").join("\n").contains("At this pace, runs out <0.1"));
        w.quota_role = Some(crate::usage_insights::QuotaRole::Scoped);
        assert!(insight_lines(&w, "pt-BR").is_empty());
        w.fresh = false;
        assert!(insight_tooltip(&w, "pt-BR").is_none());
    }

    #[test]
    fn a_reading_reads_as_the_card_prints_it() {
        assert_eq!(pct(0.0), "0");
        assert_eq!(pct(0.0004), "<0.1");
        assert_eq!(pct(0.006), "0.6");
        assert_eq!(pct(0.615), "62");
        assert_eq!(pct(0.9995), ">99.9");
        assert_eq!(pct(1.0), "100");
    }

    #[test]
    fn a_window_says_both_ends_and_when_it_resets() {
        let w = window("Current session", 0.61, Some(60 * MIN));
        assert_eq!(
            window_line(&w, MIN, "en"),
            "Current session: 61% Used · 39% left · Resets in 59 min"
        );
        assert_eq!(
            window_line(&w, MIN, "ru"),
            "Текущий сеанс: Использовано 61% · осталось 39% · Сброс через 59 мин"
        );
    }

    #[test]
    fn brazilian_portuguese_formats_usage_and_limits() {
        let w = window("Current session", 0.61, Some(60 * MIN));
        assert_eq!(
            window_line(&w, MIN, "pt-BR"),
            "Sessão atual: 61% usado · 39% restante · Renova em 59 min"
        );
        assert_eq!(label("Weekly limit", "pt-BR"), "Limite semanal");
        assert_eq!(label("Monthly limit", "pt-BR"), "Limite mensal");
        assert_eq!(label("Included usage", "pt-BR"), "Uso incluído");
        assert_eq!(label("API usage", "pt-BR"), "Uso da API");
        assert_eq!(ago(0, 20 * MIN, "pt-BR"), "há 20 min");
    }

    #[test]
    fn a_reset_is_minutes_then_hours_then_days() {
        assert_eq!(reset_text(59 * MIN, 0, "en"), "Resets in 59 min");
        assert_eq!(reset_text(135 * MIN, 0, "en"), "Resets in 2h 15m");
        assert_eq!(reset_text(3 * 1440 * MIN + 4 * 60 * MIN, 0, "en"), "Resets in 3 Days 4h");
        assert_eq!(reset_text(1440 * MIN + 60 * MIN, 0, "en"), "Resets in 1 Day 1h");
        assert_eq!(reset_text(0, MIN, "en"), "Resetting…");
    }

    #[test]
    fn a_count_window_says_how_many_rather_than_a_percentage() {
        let mut w = window("Requests today", 0.0, None);
        w.count = Some(42);
        assert_eq!(window_line(&w, 0, "en"), "Requests today: ~42");
    }

    #[test]
    fn a_header_carries_the_age_only_once_the_reading_is_old() {
        assert_eq!(header("Claude", Some(0.61), None, 0, "en"), "Claude — 61%");
        assert_eq!(header("Claude", Some(0.61), Some(0), 20 * MIN, "en"), "Claude — 61% · 20m ago");
        assert_eq!(header("Cursor", None, None, 0, "en"), "Cursor — —");
    }

    /// A reading taken seconds ago says nothing about its age, however it is flagged.
    #[test]
    fn an_age_under_a_minute_is_left_unsaid() {
        assert_eq!(header("Claude", Some(0.07), Some(0), 30_000, "en"), "Claude — 7%");
        assert_eq!(header("Claude", Some(0.07), Some(0), 90_000, "en"), "Claude — 7% · 1m ago");
    }

    /// Antigravity reports lanes below one percent; rounding the header to 1 % while the line under
    /// it says 0.5 % has the menu disagreeing with itself.
    #[test]
    fn a_header_under_one_percent_keeps_its_tenth() {
        assert_eq!(header("Antigravity", Some(0.005), None, 0, "en"), "Antigravity — 0.5%");
        let w = window("Weekly Limit", 0.005, None);
        assert_eq!(window_line(&w, 0, "en"), "Weekly Limit: 0.5% Used · 99.5% left");
    }

    #[test]
    fn korean_readings_include_usage_and_reset_times() {
        let w = window("Current session", 0.61, Some(60 * MIN));
        assert_eq!(window_line(&w, MIN, "ko"), "현재 세션: 61% 사용 · 39% 남음 · 59분 후 재설정");
        assert_eq!(reset_text(MIN, MIN, "ko"), "재설정 중…");
        assert_eq!(reset_text(136 * MIN, MIN, "ko"), "2시간 15분 후 재설정");
        assert_eq!(reset_text((3 * 1440 + 4 * 60 + 1) * MIN, MIN, "ko"), "3일 4시간 후 재설정");
        assert_eq!(ago(MIN, 21 * MIN, "ko"), "20분 전");
        assert_eq!(ago(MIN, 121 * MIN, "ko"), "2시간 전");
        assert_eq!(used_left(&window("", 0.006, None), "ko"), "0.6% 사용 · 99.4% 남음");
        assert_eq!(label("A future limit", "ko"), "A future limit");
    }

    #[test]
    fn a_fresh_reading_is_not_called_stale() {
        let fresh = UsageSnapshot { status: "ok".into(), fetched_at: 1, ..Default::default() };
        assert_eq!(stale_since(&fresh, 2 * MIN), None);
        // Ten minutes is inside the Mac's fifteen: a provider polled at 1 and 5 is not stale here
        assert_eq!(stale_since(&fresh, 10 * MIN), None);
        assert_eq!(stale_since(&fresh, 20 * MIN), Some(1));
        let flagged = UsageSnapshot { status: "stale".into(), fetched_at: 1, ..Default::default() };
        assert_eq!(stale_since(&flagged, 2), Some(1));
    }

    /// The page translates these same names; if either side is reworded the menu and the card would
    /// disagree about the same window.
    #[test]
    fn the_card_and_the_menu_agree_on_window_names() {
        let page = include_str!("../ui/notch.html");
        for name in ["Current session", "Weekly (all models)", "Weekly (Opus)", "Weekly (model-scoped)",
                     "Weekly limit", "Monthly limit", "Included usage", "API usage",
                     "Premium requests", "Chat requests", "Completions"] {
            assert!(page.contains(&format!("'{name}'")), "notch.html no longer names {name:?}");
            assert_ne!(label(name, "ru"), name, "{name:?} lost its Russian here");
            let korean = label(name, "ko");
            assert_ne!(korean, name, "{name:?} lost its Korean here");
            assert!(page.contains(&format!("'{name}':'{korean}'")), "Korean card and tray disagree on {name:?}");
        }
    }
}
