//! 关键字规则引擎。
//!
//! 每 30 秒扫描一次节目单。命中的节目按规则动作处理：
//! - `record`：建定时录制任务；
//! - `start`：打开频道的回看缓冲；
//! - `stop`：关闭回看缓冲。
//!
//! 规则只扫描生成的节目单窗口（`[epg].future_days`，最多 48 小时）。
//! 条件对节目标题做大小写不敏感的子串匹配。

use std::{sync::Arc, time::Duration};

use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

use crate::{
    epg,
    epg_store::{Condition, Rule},
    schedule::{self, Task},
    state::AppState,
};

/// 规则最多扫描 48 小时。
const MAX_LOOKAHEAD_SECS: i64 = 48 * 3_600;
/// 开始/停止动作的生效窗口。防止一个节目被反复触发。
const ACTION_WINDOW_SECS: i64 = 180;

/// 实际可扫描窗口。受生成的节目单范围限制。
fn lookahead_secs(config: &crate::config::Config) -> i64 {
    (config.epg.future_days.max(0) * 86_400).min(MAX_LOOKAHEAD_SECS)
}

/// 起规则引擎（每 30 秒扫描）。
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            if let Err(error) = tick(&state).await {
                tracing::warn!(%error, "EPG 关键字规则执行失败");
            }
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
}

async fn tick(state: &Arc<AppState>) -> Result<()> {
    let config = state.snapshot();
    if !config.epg.enabled {
        return Ok(());
    }
    let rules: Vec<Rule> = state
        .epg_store
        .rules()
        .into_iter()
        .filter(|rule| rule.enabled)
        .collect();
    if rules.is_empty() {
        return Ok(());
    }
    let Some(data) = epg::load_generated_cached(state) else {
        return Ok(());
    };
    let now = epg::now_secs() as i64;
    let lookahead = lookahead_secs(&config);
    for rule in rules {
        let Some(channel) = channel_of(&config, &rule.gid, &rule.cid) else {
            continue;
        };
        let out_id = epg::channel_out_id(channel);
        for programme in data
            .programmes
            .iter()
            .filter(|programme| programme.channel.eq_ignore_ascii_case(&out_id))
        {
            let Some(start) = epg::parse_time(&programme.start) else {
                continue;
            };
            let Some(stop) = epg::parse_time(&programme.stop) else {
                continue;
            };
            if start > now + lookahead {
                continue;
            }
            // 停止动作要等节目结束后才触发，所以允许刚结束的节目进入循环。
            let ended = stop <= now;
            if ended && (rule.action != "stop" || now >= stop + ACTION_WINDOW_SECS) {
                continue;
            }
            let title = title_of(programme);
            if !matches(&rule.condition, &title) {
                continue;
            }
            match rule.action.as_str() {
                "record" => create_record_task(state, &rule, channel, programme),
                "start" => apply_livestream(state, &rule, channel, start, stop, true, now),
                "stop" => apply_livestream(state, &rule, channel, start, stop, false, now),
                _ => {}
            }
        }
    }
    Ok(())
}

fn channel_of<'a>(
    config: &'a crate::config::Config,
    gid: &str,
    cid: &str,
) -> Option<&'a crate::config::model::Channel> {
    config
        .group(gid)?
        .channels
        .iter()
        .find(|channel| channel.id == cid)
}

fn title_of(programme: &epg::Programme) -> String {
    programme
        .texts
        .iter()
        .find(|(name, _, _)| name == "title")
        .map(|(_, _, text)| text.clone())
        .unwrap_or_default()
}

fn create_record_task(
    state: &Arc<AppState>,
    rule: &Rule,
    channel: &crate::config::model::Channel,
    programme: &epg::Programme,
) {
    let Some(start) = epg::parse_time(&programme.start) else {
        return;
    };
    let Some(stop) = epg::parse_time(&programme.stop) else {
        return;
    };
    let now = epg::now_secs() as i64;
    let offset = state.snapshot().server.timezone_offset_hours as i64;
    let recorded_start = start - rule.lead_secs.max(0);
    let recorded_end = stop + rule.tail_secs.max(0);
    if recorded_end <= now {
        return;
    }
    if state.schedule.has_rule_task(&rule.id, &programme.start) {
        return;
    }
    let title = title_of(programme);
    let template = rule.name_template.trim();
    let template = if template.is_empty() {
        "{program.title} {datetime}"
    } else {
        template
    };
    let name = render_template(template, &title, &channel.name, start, offset);
    let task = Task {
        name,
        group_id: rule.gid.clone(),
        channel_id: rule.cid.clone(),
        start_time: schedule::format_local(recorded_start, offset),
        end_time: schedule::format_local(recorded_end, offset),
        format: "mkv".into(),
        status: "pending".into(),
        note: format!("关键字规则：{}", rule.name),
        rule_id: rule.id.clone(),
        programme_start: programme.start.clone(),
        output_dir: rule.output_dir.clone(),
        ..Task::default()
    };
    match state.schedule.add(task) {
        Ok(_) => tracing::info!(
            rule = %rule.name,
            channel = %channel.name,
            programme = %title,
            "关键字规则已建录制任务"
        ),
        Err(error) => tracing::warn!(
            rule = %rule.name,
            %error,
            "关键字规则建录制任务失败"
        ),
    }
}

fn apply_livestream(
    state: &Arc<AppState>,
    rule: &Rule,
    channel: &crate::config::model::Channel,
    start: i64,
    stop: i64,
    desired: bool,
    now: i64,
) {
    let in_window = if desired {
        now >= start && now < start + ACTION_WINDOW_SECS && now < stop
    } else {
        now >= stop && now < stop + ACTION_WINDOW_SECS
    };
    if !in_window || channel.livestream == desired {
        return;
    }
    let gid = rule.gid.clone();
    let cid = rule.cid.clone();
    if let Err(error) = state.mutate(|config| {
        let Some(group) = config.groups.iter_mut().find(|group| group.id == gid) else {
            return Ok(());
        };
        let Some(channel) = group.channels.iter_mut().find(|channel| channel.id == cid) else {
            return Ok(());
        };
        channel.livestream = desired;
        Ok(())
    }) {
        tracing::warn!(%error, "关键字规则切换回看失败");
        return;
    }
    if desired {
        state.timeshift.ensure(state, &rule.gid, &rule.cid);
    }
    tracing::info!(
        rule = %rule.name,
        channel = %channel.name,
        action = if desired { "start" } else { "stop" },
        "关键字规则已执行"
    );
}

/// 条件树命中。
pub fn matches(condition: &Condition, title: &str) -> bool {
    let title = title.to_lowercase();
    let has_leaf = !condition.keywords.is_empty() || !condition.groups.is_empty();
    if !has_leaf {
        return false;
    }
    let keyword_hit = |keyword: &String| {
        let keyword = keyword.trim().to_lowercase();
        !keyword.is_empty() && title.contains(&keyword)
    };
    let any_group = || condition.groups.iter().any(|group| matches(group, &title));
    let all_groups = || condition.groups.iter().all(|group| matches(group, &title));
    match condition.op.as_str() {
        "any" => condition.keywords.iter().any(keyword_hit) || any_group(),
        "not" => !condition.keywords.iter().any(keyword_hit) && !any_group(),
        _ => condition.keywords.iter().all(keyword_hit) && all_groups(),
    }
}

/// 展开任务名模板。
///
/// 支持 `{program.title}`、`{channel.name}`、`{date}`、`{time}`、`{datetime}`。
/// 时间占位符可带 UTC 偏移后缀，例如 `{datetime+8}`、`{date-5}`、`{time+5:30}`。
pub fn render_template(
    template: &str,
    title: &str,
    channel_name: &str,
    start_epoch: i64,
    offset_hours: i64,
) -> String {
    let mut out = String::with_capacity(template.len() + 32);
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let Some(close) = rest[open..].find('}') else {
            out.push_str(&rest[open..]);
            return out;
        };
        let token = &rest[open + 1..open + close];
        let (name, shift) = split_shift(token);
        let value = match name {
            "program.title" => Some(title.to_string()),
            "channel.name" => Some(channel_name.to_string()),
            "date" | "time" | "datetime" => {
                let local = schedule::format_local(start_epoch + shift, offset_hours);
                Some(match name {
                    "date" => local.chars().take(10).collect(),
                    "time" => local.chars().skip(11).take(5).collect(),
                    _ => local.replace('T', " "),
                })
            }
            _ => None,
        };
        match value {
            Some(value) => out.push_str(&value),
            None => {
                out.push('{');
                out.push_str(token);
                out.push('}');
            }
        }
        rest = &rest[open + close + 1..];
    }
    out.push_str(rest);
    if out.trim().is_empty() {
        title.to_string()
    } else {
        out
    }
}

/// 拆分时间占位符的偏移后缀。返回 `(名字, 偏移秒数)`。
fn split_shift(token: &str) -> (&str, i64) {
    let Some(index) = token.find(['+', '-']) else {
        return (token, 0);
    };
    let (name, rest) = token.split_at(index);
    let sign = if rest.starts_with('-') { -1 } else { 1 };
    let rest = &rest[1..];
    let seconds = if let Some((hours, minutes)) = rest.split_once(':') {
        let hours: i64 = hours.parse().unwrap_or(0);
        let minutes: i64 = minutes.parse().unwrap_or(0);
        hours * 3_600 + minutes * 60
    } else {
        rest.parse::<i64>().unwrap_or(0) * 3_600
    };
    (name, sign * seconds)
}

/// 校验规则频道与动作。
pub fn validate(state: &AppState, rule: &Rule) -> Result<()> {
    let config = state.snapshot();
    if channel_of(&config, &rule.gid, &rule.cid).is_none() {
        bail!("频道不存在");
    }
    if !matches!(rule.action.as_str(), "record" | "start" | "stop") {
        bail!("未知动作");
    }
    if !condition_has_leaf(&rule.condition) {
        bail!("匹配条件不能为空");
    }
    Ok(())
}

fn condition_has_leaf(condition: &Condition) -> bool {
    condition
        .keywords
        .iter()
        .any(|keyword| !keyword.trim().is_empty())
        || condition.groups.iter().any(condition_has_leaf)
}

/// 预览节目单窗口内的命中节目。
pub fn preview(state: &AppState, rule: &Rule) -> Result<Value> {
    let config = state.snapshot();
    if !config.epg.enabled {
        bail!("EPG 未开启");
    }
    let channel = channel_of(&config, &rule.gid, &rule.cid).context("频道不存在")?;
    let out_id = epg::channel_out_id(channel);
    let binding = state.epg_store.binding(&rule.gid, &rule.cid);
    let data = epg::load_generated_cached(state).context("节目单尚未生成，请先采集")?;
    let now = epg::now_secs() as i64;
    let lookahead = lookahead_secs(&config);
    let offset = config.server.timezone_offset_hours as i64;
    let mut matches_out = Vec::new();
    for programme in data
        .programmes
        .iter()
        .filter(|programme| programme.channel.eq_ignore_ascii_case(&out_id))
    {
        let Some(start) = epg::parse_time(&programme.start) else {
            continue;
        };
        let Some(stop) = epg::parse_time(&programme.stop) else {
            continue;
        };
        if stop <= now || start > now + lookahead {
            continue;
        }
        let title = title_of(programme);
        if !matches(&rule.condition, &title) {
            continue;
        }
        matches_out.push(json!({
            "title": title,
            "start": programme.start,
            "stop": programme.stop,
            "start_epoch": start,
            "stop_epoch": stop,
            "disp_start": schedule::format_local(start, offset),
            "disp_end": schedule::format_local(stop, offset),
            "duration_secs": (stop - start).max(0),
        }));
    }
    matches_out.sort_by_key(|item| item["start_epoch"].as_i64().unwrap_or(0));
    Ok(json!({
        "ok": true,
        "channel": channel.name,
        "channel_id": out_id,
        "bound": binding.is_some(),
        "binding": binding,
        "lookahead_secs": lookahead,
        "matches": matches_out,
        "total": matches_out.len(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::epg_store::Condition;

    fn condition(op: &str, keywords: &[&str], groups: Vec<Condition>) -> Condition {
        Condition {
            op: op.into(),
            keywords: keywords.iter().map(|value| value.to_string()).collect(),
            groups,
        }
    }

    #[test]
    fn all_any_not_match() {
        let all = condition("all", &["直播", "利物浦"], Vec::new());
        assert!(matches(&all, "英超直播：利物浦 vs 曼联"));
        assert!(!matches(&all, "英超直播：阿森纳 vs 曼联"));

        let any = condition("any", &[], vec![condition("all", &["利物浦"], Vec::new())]);
        assert!(matches(&any, "利物浦比赛"));
        assert!(!matches(&any, "阿森纳比赛"));

        let not = condition("not", &["广告"], Vec::new());
        assert!(matches(&not, "新闻联播"));
        assert!(!matches(&not, "广告时间"));
    }

    #[test]
    fn empty_condition_never_matches() {
        assert!(!matches(&Condition::default(), "任意节目"));
    }

    #[test]
    fn template_renders_offsets() {
        // 2026-01-01 00:00:00 UTC。
        let epoch = 1_767_225_600;
        let text = render_template(
            "{program.title}/{channel.name}/{date} {time} {datetime+8}",
            "比赛",
            "CCTV5",
            epoch,
            0,
        );
        assert!(text.starts_with("比赛/CCTV5/"));
        assert!(text.contains("2026-01-01"));
        assert!(text.contains("08:00"));
    }

    #[test]
    fn lookahead_is_capped_by_generated_window() {
        let mut config = crate::config::Config::default();
        config.epg.future_days = 1;
        assert_eq!(lookahead_secs(&config), 86_400);
        config.epg.future_days = 5;
        assert_eq!(lookahead_secs(&config), 48 * 3_600);
        config.epg.future_days = 0;
        assert_eq!(lookahead_secs(&config), 0);
    }

    #[test]
    fn template_keeps_unknown_tokens() {
        let text = render_template("{unknown} {date}", "x", "y", 0, 0);
        assert!(text.contains("{unknown}"));
    }
}
