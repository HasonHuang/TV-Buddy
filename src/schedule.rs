//! 定时录制：任务存储、调度与切片抓取。
//!
//! 证据：F-009 §2（S-REC-02）。
//! 任务存在 `<cache>/schedule.json`；产物在 `<cache>/recordings/`。
//! HLS 录制按上游媒体清单增量下载切片并合并；`format=mkv` 且有 ffmpeg 时再转封装。
//! DASH 录制增量抓取各轨 init/切片，结束后交给 ffmpeg 并轨为 MKV。

use std::{
    collections::{HashMap, HashSet},
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::state::AppState;

/// 任务。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    pub id: String,
    pub name: String,
    pub group_id: String,
    pub channel_id: String,
    pub start_time: String,
    pub end_time: String,
    pub format: String,
    pub status: String,
    pub output_file: String,
    pub extra_files: Vec<String>,
    pub note: String,
    pub error: String,
    pub created_at: String,
    pub file_size: u64,
    pub record_secs: u64,
}

impl Default for Task {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            group_id: String::new(),
            channel_id: String::new(),
            start_time: String::new(),
            end_time: String::new(),
            format: "mkv".into(),
            status: "pending".into(),
            output_file: String::new(),
            extra_files: Vec::new(),
            note: String::new(),
            error: String::new(),
            created_at: String::new(),
            file_size: 0,
            record_secs: 0,
        }
    }
}

/// 任务存储。
pub struct Store {
    path: PathBuf,
    recordings: PathBuf,
    offset: i64,
    tasks: Mutex<Vec<Task>>,
    running: Mutex<HashMap<String, Arc<AtomicBool>>>,
}

impl Store {
    /// 载入任务文件。
    pub fn load(cache_dir: &Path, offset: i64) -> Self {
        let path = cache_dir.join("schedule.json");
        let recordings = cache_dir.join("recordings");
        let tasks = fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<Vec<Task>>(&text).ok())
            .unwrap_or_default();
        let _ = fs::create_dir_all(&recordings);
        Self {
            path,
            recordings,
            offset,
            tasks: Mutex::new(tasks),
            running: Mutex::new(HashMap::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<Task>> {
        self.tasks.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn persist(&self, tasks: &[Task]) {
        let text = serde_json::to_string_pretty(tasks).unwrap_or_else(|_| "[]".into());
        let temp = self.path.with_extension("json.tmp");
        if fs::write(&temp, text).is_ok() {
            let _ = fs::rename(&temp, &self.path);
        }
    }

    pub fn list(&self) -> Vec<Task> {
        self.lock().clone()
    }

    /// 新增任务；返回任务 ID。
    pub fn add(&self, mut task: Task) -> Result<String> {
        if task.group_id.trim().is_empty() || task.channel_id.trim().is_empty() {
            bail!("缺少 group_id 或 channel_id");
        }
        if parse_local(&task.start_time, self.offset).is_none()
            || parse_local(&task.end_time, self.offset).is_none()
        {
            bail!("时间格式应为 YYYY-MM-DDTHH:MM:SS");
        }
        if task.id.is_empty() {
            task.id = uuid::Uuid::new_v4().to_string();
            task.created_at = format_local(now_epoch(), self.offset);
        }
        let mut tasks = self.lock();
        tasks.push(task.clone());
        self.persist(&tasks);
        Ok(task.id)
    }

    /// 按下标更新允许的字段。
    pub fn update(&self, index: usize, body: &Value) -> Result<Task> {
        let mut tasks = self.lock();
        let task = tasks.get_mut(index).context("任务不存在")?;
        for (key, value) in body.as_object().into_iter().flatten() {
            let Some(text) = value.as_str() else { continue };
            match key.as_str() {
                "name" => task.name = text.to_string(),
                "group_id" => task.group_id = text.to_string(),
                "channel_id" => task.channel_id = text.to_string(),
                "start_time" => task.start_time = text.to_string(),
                "end_time" => task.end_time = text.to_string(),
                "format" => task.format = text.to_string(),
                "status" => task.status = text.to_string(),
                "note" => task.note = text.to_string(),
                _ => {}
            }
        }
        let cloned = task.clone();
        self.persist(&tasks);
        Ok(cloned)
    }

    /// 删除任务；返回 `(任务, 是否在录制)`。
    pub fn delete(&self, index: usize) -> Result<(Task, bool)> {
        let mut tasks = self.lock();
        if index >= tasks.len() {
            bail!("任务不存在");
        }
        let task = tasks.remove(index);
        self.persist(&tasks);
        let was_running = self.stop_flag(&task.id);
        Ok((task, was_running))
    }

    /// 停止任务；返回是否在录制。
    pub fn stop(&self, index: usize) -> Result<bool> {
        let tasks = self.lock();
        let task = tasks.get(index).context("任务不存在")?;
        Ok(self.stop_flag(&task.id))
    }

    fn stop_flag(&self, id: &str) -> bool {
        let running = self.running.lock().unwrap_or_else(|e| e.into_inner());
        match running.get(id) {
            Some(flag) => {
                flag.store(true, Ordering::SeqCst);
                true
            }
            None => false,
        }
    }

    fn begin(&self, id: &str) -> Arc<AtomicBool> {
        let flag = Arc::new(AtomicBool::new(false));
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id.to_string(), flag.clone());
        flag
    }

    fn finish(&self, id: &str) {
        self.running
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(id);
    }

    fn set(&self, id: &str, apply: impl FnOnce(&mut Task)) {
        let mut tasks = self.lock();
        if let Some(task) = tasks.iter_mut().find(|task| task.id == id) {
            apply(task);
        }
        self.persist(&tasks);
    }

    /// 产物路径（下载用）；拒绝路径穿越。
    pub fn file(&self, name: &str) -> Option<PathBuf> {
        let safe = crate::logo::sanitize(name)?;
        let path = self.recordings.join(safe);
        path.is_file().then_some(path)
    }

    /// 时区偏移（小时）。
    pub fn offset(&self) -> i64 {
        self.offset
    }

    /// 下一个到期的 pending 任务下标。
    fn due(&self, now: i64) -> Option<usize> {
        let tasks = self.lock();
        tasks.iter().position(|task| {
            task.status == "pending"
                && parse_local(&task.start_time, self.offset)
                    .map(|at| at <= now)
                    .unwrap_or(false)
        })
    }
}

/// 起调度任务（每 10 秒扫描）。
pub fn spawn(state: Arc<AppState>) {
    tokio::spawn(async move {
        loop {
            let now = now_epoch();
            while let Some(index) = state.schedule.due(now) {
                let tasks = state.schedule.list();
                let Some(task) = tasks.get(index).cloned() else {
                    break;
                };
                let end = parse_local(&task.end_time, state.schedule.offset()).unwrap_or(now);
                if now >= end {
                    state.schedule.set(&task.id, |task| {
                        task.status = "failed".into();
                        task.note = "错过录制区间".into();
                    });
                    continue;
                }
                state
                    .schedule
                    .set(&task.id, |task| task.status = "recording".into());
                let flag = state.schedule.begin(&task.id);
                let state = state.clone();
                tokio::spawn(async move {
                    record(state.clone(), task.clone(), end, flag).await;
                    state.schedule.finish(&task.id);
                });
            }
            tokio::time::sleep(Duration::from_secs(10)).await;
        }
    });
}

/// 录制一个任务。
async fn record(state: Arc<AppState>, task: Task, end: i64, stop: Arc<AtomicBool>) {
    let started = Instant::now();
    let mut seen: HashSet<String> = HashSet::new();
    let mut merged: Vec<u8> = Vec::new();
    let mut note = String::new();
    let config = state.snapshot();
    let Some(group) = config.group(&task.group_id) else {
        return fail(&state, &task, "Group not found");
    };
    let Some(channel) = group.channels.iter().find(|c| c.id == task.channel_id) else {
        return fail(&state, &task, "Channel not found");
    };
    if group.mode == "dash" {
        return record_dash(&state, &task, end, &stop, channel).await;
    }
    let mut url = channel.url.clone();
    let headers =
        crate::http::routes::stream::channel_headers(&state, &task.group_id, &task.channel_id);

    while now_epoch() < end && !stop.load(Ordering::SeqCst) {
        let mut request = state.http_for(&task.group_id, &task.channel_id).get(&url);
        for (name, value) in &headers {
            request = request.header(name, value);
        }
        let text = match request.send().await {
            Ok(response) if response.status().is_success() => match response.text().await {
                Ok(text) => text,
                Err(error) => {
                    note = format!("读取清单失败: {error}");
                    break;
                }
            },
            Ok(response) => {
                note = format!("上游 {}", response.status());
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
            Err(error) => {
                note = format!("上游错误: {error}");
                tokio::time::sleep(Duration::from_secs(2)).await;
                continue;
            }
        };
        if crate::proxy::media::is_master(&text) {
            if let Some((_, variant)) = crate::proxy::media::best_variant(&text, &url) {
                url = variant;
                continue;
            }
        }
        let base = url.clone();
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let Ok(segment) = url::Url::parse(&base).and_then(|base| base.join(line)) else {
                continue;
            };
            let segment = segment.to_string();
            if !seen.insert(segment.clone()) {
                continue;
            }
            let mut request = state
                .http_for(&task.group_id, &task.channel_id)
                .get(&segment);
            for (name, value) in &headers {
                request = request.header(name, value);
            }
            if let Ok(response) = request.send().await {
                if response.status().is_success() {
                    if let Ok(bytes) = response.bytes().await {
                        merged.extend_from_slice(&bytes);
                    }
                }
            }
        }
        let interval = crate::proxy::media::target_duration(&text)
            .unwrap_or(5)
            .clamp(2, 10);
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }

    let record_secs = started.elapsed().as_secs();
    if merged.is_empty() {
        let reason = if note.is_empty() {
            "录制区间内无可用切片".to_string()
        } else {
            note
        };
        return fail(&state, &task, &reason);
    }
    let stamp = now_epoch();
    let stem = format!(
        "{}_{}",
        crate::logo::sanitize(&task.name).unwrap_or_else(|| "record".into()),
        stamp
    );
    let dir = Path::new(&state.cache_dir).join("recordings");
    let _ = fs::create_dir_all(&dir);
    let ts = dir.join(format!("{stem}.ts"));
    if let Err(error) = fs::write(&ts, &merged) {
        return fail(&state, &task, &format!("写产物失败: {error}"));
    }
    let mut output = ts.clone();
    let mut format = "ts".to_string();
    if task.format.eq_ignore_ascii_case("mkv") {
        let mkv = dir.join(format!("{stem}.mkv"));
        let status = tokio::process::Command::new("ffmpeg")
            .args(["-y", "-loglevel", "error", "-i"])
            .arg(&ts)
            .args(["-map", "0", "-c", "copy"])
            .arg(&mkv)
            .status()
            .await;
        if matches!(status, Ok(status) if status.success()) && mkv.is_file() {
            let _ = fs::remove_file(&ts);
            output = mkv;
            format = "mkv".into();
        } else {
            note = "ffmpeg 不可用，保留 TS".into();
        }
    }
    let size = fs::metadata(&output).map(|meta| meta.len()).unwrap_or(0);
    let name = output
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    state.schedule.set(&task.id, |task| {
        task.status = "finished".into();
        task.output_file = name;
        task.file_size = size;
        task.record_secs = record_secs;
        task.note = if note.is_empty() {
            format!("{format} 录制完成")
        } else {
            note.clone()
        };
    });
}

/// DASH 录制：按轨道增量抓取 init/切片，结束后用 ffmpeg 并轨为 MKV。
///
/// 目标在 DASH 录制时先并 `_merged_video_*` / `_merged_audio-*`，再
/// `ffmpeg -c copy` 出 MKV（F-009 §2.3）。静态 MPD 与目标一样报
/// `录制区间内无可用切片`。
async fn record_dash(
    state: &Arc<AppState>,
    task: &Task,
    end: i64,
    stop: &AtomicBool,
    channel: &crate::config::model::Channel,
) {
    let started = Instant::now();
    if stop.load(Ordering::SeqCst) {
        return fail(state, task, "录制已停止");
    }
    let url = match crate::resolver::resolve(state, &task.group_id, &task.channel_id, channel).await
    {
        Ok(Some(url)) => url,
        Ok(None) => channel.url.clone(),
        Err(error) => return fail(state, task, &format!("解析脚本失败: {error:#}")),
    };
    let mut presentation = match fetch_presentation(state, task, &url).await {
        Ok(presentation) => presentation,
        Err(error) => {
            let reason = if error.contains("没有可转封装的轨道") {
                "录制区间内无可用切片".to_string()
            } else {
                error
            };
            return fail(state, task, &reason);
        }
    };
    if !presentation.is_live {
        return fail(state, task, "录制区间内无可用切片");
    }
    let probe = tokio::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .await;
    if !matches!(probe, Ok(output) if output.status.success()) {
        return fail(state, task, "ffmpeg 不可用");
    }
    let dir = Path::new(&state.cache_dir).join("recordings");
    if let Err(error) = fs::create_dir_all(&dir) {
        return fail(state, task, &format!("建录制目录失败: {error}"));
    }
    let temp = dir.join(format!("tmp-{}", task.id));
    let _ = fs::remove_dir_all(&temp);
    if let Err(error) = fs::create_dir_all(&temp) {
        return fail(state, task, &format!("建临时目录失败: {error}"));
    }

    struct Part {
        kind: String,
        path: PathBuf,
        initialized: bool,
        seen: HashSet<(u64, u64)>,
    }
    let mut parts: HashMap<String, Part> = HashMap::new();
    let mut note = String::new();
    loop {
        for stream in presentation
            .streams
            .iter()
            .filter(|stream| stream.kind == "video" || stream.kind == "audio")
        {
            let name = if stream.name.is_empty() {
                stream.id.clone()
            } else {
                stream.name.clone()
            };
            let part = parts.entry(name.clone()).or_insert_with(|| Part {
                kind: stream.kind.clone(),
                path: temp.join(format!("{name}.mp4")),
                initialized: false,
                seen: HashSet::new(),
            });
            if !part.initialized {
                if let Some(init) = stream.init.first() {
                    match crate::http::routes::stream::fetch_bytes(
                        state,
                        init,
                        &task.group_id,
                        &task.channel_id,
                    )
                    .await
                    {
                        Ok(bytes) => {
                            if let Err(error) = append(&part.path, &bytes) {
                                note = format!("写 init 失败: {error}");
                            } else {
                                part.initialized = true;
                            }
                        }
                        Err(error) => note = format!("抓 init 失败: {error}"),
                    }
                } else {
                    part.initialized = true;
                }
            }
            for segment in &stream.segments {
                let key = (segment.number, segment.time);
                if part.seen.contains(&key) {
                    continue;
                }
                for segment_url in &segment.urls {
                    match crate::http::routes::stream::fetch_bytes(
                        state,
                        segment_url,
                        &task.group_id,
                        &task.channel_id,
                    )
                    .await
                    {
                        Ok(bytes) => {
                            if let Err(error) = append(&part.path, &bytes) {
                                note = format!("写切片失败: {error}");
                                break;
                            }
                            part.seen.insert(key);
                            break;
                        }
                        Err(error) => note = format!("抓切片失败: {error}"),
                    }
                }
            }
        }
        if stop.load(Ordering::SeqCst) || now_epoch() >= end {
            break;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        match fetch_presentation(state, task, &url).await {
            Ok(next) if next.is_live => presentation = next,
            Ok(_) => note = "上游 MPD 已转为静态".into(),
            Err(error) => note = error,
        }
    }

    let mut video = None;
    let mut audio = None;
    for part in parts.values() {
        if !part.path.is_file() || fs::metadata(&part.path).map(|m| m.len()).unwrap_or(0) == 0 {
            continue;
        }
        match part.kind.as_str() {
            "video" if video.is_none() => video = Some(part.path.clone()),
            "audio" if audio.is_none() => audio = Some(part.path.clone()),
            _ => {}
        }
    }
    let Some(video) = video else {
        let _ = fs::remove_dir_all(&temp);
        let reason = if note.is_empty() {
            "录制区间内无可用切片".to_string()
        } else {
            note
        };
        return fail(state, task, &reason);
    };

    let stamp = now_epoch();
    let stem = format!(
        "{}_{}",
        crate::logo::sanitize(&task.name).unwrap_or_else(|| "record".into()),
        stamp
    );
    let output = dir.join(format!("{stem}.mkv"));
    let mut command = tokio::process::Command::new("ffmpeg");
    command.kill_on_drop(true);
    command.args(["-y", "-loglevel", "error", "-i"]).arg(&video);
    if let Some(audio) = &audio {
        command.arg("-i").arg(audio);
    }
    if audio.is_some() {
        command.args(["-map", "0:v:0", "-map", "1:a:0"]);
    } else {
        command.args(["-map", "0"]);
    }
    command.args(["-c", "copy"]).arg(&output);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            let _ = fs::remove_dir_all(&temp);
            return fail(state, task, &format!("ffmpeg 不可用: {error}"));
        }
    };
    let merged = tokio::time::timeout(Duration::from_secs(120), child.wait()).await;
    let ok = match merged {
        Ok(Ok(status)) => status.success(),
        _ => {
            let _ = child.start_kill();
            let _ = child.wait().await;
            false
        }
    };
    if !ok || !output.is_file() {
        let _ = fs::remove_file(&output);
        let _ = fs::remove_dir_all(&temp);
        return fail(state, task, "ffmpeg 并轨失败");
    }
    let record_secs = started.elapsed().as_secs();
    let size = fs::metadata(&output).map(|meta| meta.len()).unwrap_or(0);
    let name = output
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default();
    let _ = fs::remove_dir_all(&temp);
    state.schedule.set(&task.id, |task| {
        task.status = "finished".into();
        task.output_file = name;
        task.file_size = size;
        task.record_secs = record_secs;
        task.note = "mkv 录制完成".into();
    });
}

/// 抓取并解析当前 MPD。
async fn fetch_presentation(
    state: &AppState,
    task: &Task,
    url: &str,
) -> Result<crate::dash::Presentation, String> {
    let text = crate::http::routes::stream::fetch(state, url, &task.group_id, &task.channel_id)
        .await
        .map_err(|error| format!("读取 MPD 失败: {error}"))?;
    crate::dash::parse_presentation(&text, url).map_err(|error| format!("解析 MPD 失败: {error:#}"))
}

/// 追加写入切片文件。
fn append(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(bytes)
}

fn fail(state: &AppState, task: &Task, reason: &str) {
    state.schedule.set(&task.id, |task| {
        task.status = "failed".into();
        task.error = reason.to_string();
    });
}

/// 当前 Unix 秒。
pub fn now_epoch() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or(0)
}

/// 解析本地时间 `YYYY-MM-DDTHH:MM:SS`；`offset_hours` 为东区偏移。
pub fn parse_local(text: &str, offset_hours: i64) -> Option<i64> {
    let text = text.trim().replace(' ', "T");
    let (date, time) = text.split_once('T')?;
    let mut parts = date.split('-');
    let year: i64 = parts.next()?.parse().ok()?;
    let month: i64 = parts.next()?.parse().ok()?;
    let day: i64 = parts.next()?.parse().ok()?;
    let mut clock = time.split(':');
    let hour: i64 = clock.next()?.parse().ok()?;
    let minute: i64 = clock.next()?.parse().ok()?;
    let second: i64 = clock.next().unwrap_or("0").parse().ok()?;
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || hour > 23 || minute > 59 {
        return None;
    }
    Some(
        days_from_civil(year, month, day) * 86_400 + hour * 3600 + minute * 60 + second
            - offset_hours * 3600,
    )
}

/// 本地时间文本（秒级）。
pub fn format_local(epoch: i64, offset_hours: i64) -> String {
    let local = epoch + offset_hours * 3600;
    let days = local.div_euclid(86_400);
    let rest = local.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// UTC ISO-8601 文本（秒级，以 `Z` 结尾）。
pub fn format_utc_iso(epoch: i64) -> String {
    let days = epoch.div_euclid(86_400);
    let rest = epoch.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        rest / 3600,
        (rest % 3600) / 60,
        rest % 60
    )
}

/// Howard Hinnant 的 `days_from_civil`。
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = year - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// `days_from_civil` 的逆。
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_roundtrip() {
        let epoch = parse_local("2026-10-20T19:24:29", 8).expect("解析");
        assert_eq!(format_local(epoch, 8), "2026-10-20T19:24:29");
        assert_eq!(parse_local("2026-10-20 19:24", 8), Some(epoch - 29));
        assert!(parse_local("2026-13-20T00:00:00", 0).is_none());
        assert!(parse_local("bad", 0).is_none());
    }

    #[test]
    fn store_add_update_delete() {
        let dir = std::env::temp_dir().join(format!("schedule-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("建目录");
        let store = Store::load(&dir, 8);
        let mut task = Task::default();
        task.name = "测试".into();
        task.group_id = "g".into();
        task.channel_id = "c".into();
        task.start_time = "2026-10-20T00:00:00".into();
        task.end_time = "2026-10-20T00:01:00".into();
        let id = store.add(task).expect("新增");
        assert_eq!(store.list().len(), 1);
        store
            .update(
                0,
                &serde_json::json!({ "name": "改名", "status": "pending" }),
            )
            .expect("更新");
        assert_eq!(store.list()[0].name, "改名");
        let (task, running) = store.delete(0).expect("删除");
        assert_eq!(task.id, id);
        assert!(!running);
        assert!(store.list().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
