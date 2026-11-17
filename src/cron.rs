//! 共享 5 段 cron 解析与触发时间计算。
//!
//! EPG 与质量调度共用本模块，避免两套行为。

use std::collections::HashSet;

use anyhow::{bail, Context, Result};

#[derive(Debug, Clone)]
struct CronField {
    values: HashSet<u32>,
    wildcard: bool,
}

impl CronField {
    fn matches(&self, value: u32) -> bool {
        self.values.contains(&value)
    }
}

fn parse_cron_field(text: &str, min: u32, max: u32, sunday_seven: bool) -> Result<CronField> {
    let mut values = HashSet::new();
    let mut wildcard = false;
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            bail!("Cron 字段为空");
        }
        let (range, step) = match part.split_once('/') {
            Some((range, step)) => {
                let step: u32 = step.parse().context("Cron 步长不合法")?;
                if step == 0 {
                    bail!("Cron 步长不能为 0");
                }
                (range, step)
            }
            None => (part, 1),
        };
        let (start, end) = if range == "*" {
            // `*/N` 是受限集合，不是通配。否则日/周字段的 OR 规则会忽略步长。
            if step == 1 {
                wildcard = true;
            }
            (min, max)
        } else if let Some((start, end)) = range.split_once('-') {
            (
                start.parse().context("Cron 起点不合法")?,
                end.parse().context("Cron 终点不合法")?,
            )
        } else {
            let value: u32 = range.parse().context("Cron 值不合法")?;
            (value, value)
        };
        if start > end || start < min || end > max {
            bail!("Cron 字段超出范围");
        }
        let mut value = start;
        while value <= end {
            let normalized = if sunday_seven && value == 7 { 0 } else { value };
            values.insert(normalized);
            value += step;
        }
    }
    if values.is_empty() {
        bail!("Cron 字段为空");
    }
    Ok(CronField { values, wildcard })
}

/// 解析后的 5 段 cron。
#[derive(Debug, Clone)]
pub struct CronSpec {
    minute: CronField,
    hour: CronField,
    day: CronField,
    month: CronField,
    weekday: CronField,
}

/// 解析标准 5 段 cron。
pub fn parse_cron(expr: &str) -> Result<CronSpec> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        bail!("需要标准 5 字段表达式");
    }
    Ok(CronSpec {
        minute: parse_cron_field(fields[0], 0, 59, false)?,
        hour: parse_cron_field(fields[1], 0, 23, false)?,
        day: parse_cron_field(fields[2], 1, 31, false)?,
        month: parse_cron_field(fields[3], 1, 12, false)?,
        weekday: parse_cron_field(fields[4], 0, 7, true)?,
    })
}

/// 指定 Unix 秒是否命中表达式。`offset_hours` 是服务器时区偏移。
pub fn cron_matches(spec: &CronSpec, epoch: i64, offset_hours: i64) -> bool {
    let local = epoch + offset_hours * 3_600;
    let days = local.div_euclid(86_400);
    let secs = local.rem_euclid(86_400);
    let (_, month, day) = civil_from_days(days);
    let weekday = (days + 4).rem_euclid(7) as u32;
    if !spec.month.matches(month as u32)
        || !spec.hour.matches((secs / 3_600) as u32)
        || !spec.minute.matches(((secs % 3_600) / 60) as u32)
    {
        return false;
    }
    let day_ok = spec.day.matches(day as u32);
    let weekday_ok = spec.weekday.matches(weekday);
    match (spec.day.wildcard, spec.weekday.wildcard) {
        (true, true) => true,
        (false, true) => day_ok,
        (true, false) => weekday_ok,
        (false, false) => day_ok || weekday_ok,
    }
}

/// 计算接下来的 `count` 个触发时间（Unix 秒）。
pub fn cron_preview(expr: &str, now: i64, offset_hours: i64, count: usize) -> Result<Vec<i64>> {
    let spec = parse_cron(expr)?;
    let mut next = Vec::new();
    let mut candidate = now.div_euclid(60) * 60 + 60;
    let limit = now + 366 * 86_400;
    while candidate <= limit && next.len() < count.clamp(1, 10) {
        if cron_matches(&spec, candidate, offset_hours) {
            next.push(candidate);
        }
        candidate += 60;
    }
    if next.is_empty() {
        bail!("一年内没有触发时间");
    }
    Ok(next)
}

/// 最近一次不晚于 `now` 的触发时间（Unix 秒）。
///
/// 从当前分钟向前查找。一年内没有触发时间时返回错误。
pub fn cron_prev(expr: &str, now: i64, offset_hours: i64) -> Result<i64> {
    let spec = parse_cron(expr)?;
    let mut candidate = now.div_euclid(60) * 60;
    let limit = now.saturating_sub(366 * 86_400);
    while candidate >= limit {
        if cron_matches(&spec, candidate, offset_hours) {
            return Ok(candidate);
        }
        candidate -= 60;
    }
    bail!("一年内没有触发时间")
}

/// 表达式是否在上次运行后已经触发。非法表达式返回 `false`。
pub fn cron_due(expr: &str, now: i64, offset_hours: i64, last_run: i64) -> bool {
    match cron_prev(expr, now, offset_hours) {
        Ok(prev) => last_run < prev,
        Err(_) => false,
    }
}

/// 公历天数 → `(年, 月, 日)`。
pub(crate) fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = if month_prime < 10 {
        month_prime + 3
    } else {
        month_prime - 9
    };
    (year + if month <= 2 { 1 } else { 0 }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cron_due_uses_last_run_and_rejects_bad_expression() {
        let next = cron_preview("0 * * * *", 1_700_000_000, 0, 1).expect("cron")[0];
        assert!(cron_due("0 * * * *", next, 0, next - 1));
        assert!(!cron_due("0 * * * *", next, 0, next));
        assert!(!cron_due("bad", next, 0, 0), "非法表达式不触发");
    }

    #[test]
    fn step_does_not_make_day_or_weekday_wildcard() {
        let stepped = parse_cron("0 0 */2 * *").expect("cron");
        assert!(!stepped.day.wildcard, "*/N 是受限集合");
        assert!(stepped.weekday.wildcard);
        let wildcard = parse_cron("0 0 * * *").expect("cron");
        assert!(wildcard.day.wildcard);
    }

    #[test]
    fn preview_and_prev_agree() {
        let now = 1_700_000_000;
        let prev = cron_prev("0 */4 * * *", now, 0).expect("prev");
        let next = cron_preview("0 */4 * * *", now, 0, 1).expect("next")[0];
        assert!(prev <= now);
        assert!(next > now);
        assert_eq!((next - prev) % 14_400, 0);
    }
}
