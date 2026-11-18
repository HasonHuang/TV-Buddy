/** 质量校验页：概览、规则、结果与来源。
 *
 * 契约见 `docs/design/16-quality-admission-system.md` §8–§9。
 * 结果页与来源页在后续任务补齐筛选与操作，这里先给出可读视图。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import {
  api,
  cancelQuality,
  deleteQualitySource,
  disableQualitySource,
  enableQualitySource,
  fetchPlaylistSources,
  fetchQualityResults,
  fetchQualitySources,
  fetchQualityStatus,
  previewQualitySchedule,
  putConfig,
  runQuality,
  syncPlaylistSource,
  updatePlaylistSource,
  type PlaylistSourceConfig,
  type QualityResult,
  type QualitySource,
  type QualityStatus,
} from './api';
import { Icon } from './icons';
import {
  Badge,
  Card,
  Confirm,
  Empty,
  Field,
  Modal,
  PageHead,
  Select,
  Switch,
  useToast,
} from './ui';

type Tab = 'overview' | 'rules' | 'results' | 'sources';
type Cfg = Record<string, unknown>;

const TABS: [Tab, string][] = [
  ['overview', '概览'],
  ['rules', '规则'],
  ['results', '结果'],
  ['sources', '来源'],
];

const ACTION_LABEL: Record<string, string> = {
  admitted: '准入',
  rejected: '拒绝',
  removed: '移出',
  kept: '保留',
  hidden: '下线',
  source_removed: '源移除',
  held: '暂停准入',
  removal_paused: '暂停移出',
  skipped: '跳过',
};

const PAUSE_LABEL: Record<string, string> = {
  guard: '安全阀已暂停',
  budget: '本轮门控时限用尽',
  probe_error: '地址变化探测错误，本轮已暂停',
};

function resultActionText(item: QualityResult): string {
  if (item.reason === 'guard') {
    return item.action === 'rejected' ? '暂停准入' : '暂停移出';
  }
  return ACTION_LABEL[item.action] ?? item.action;
}

function resultActionTone(item: QualityResult): string {
  if (item.reason === 'guard') return 'warn';
  switch (item.action) {
    case 'admitted':
      return 'ok';
    case 'removed':
    case 'rejected':
      return 'danger';
    case 'hidden':
      return 'warn';
    case 'held':
    case 'removal_paused':
      return 'warn';
    default:
      return 'muted';
  }
}

const REASON_LABEL: Record<string, string> = {
  pass: '合格',
  retained: '已保留',
  unreachable: '不可达',
  speed_low: '速率低',
  resolution_low: '分辨率低',
  resolution_high: '分辨率高',
  delay_high: '延迟高',
  supply: '兜底保留',
  probe_error: '探测错误',
  source_removed: '源已移除',
  source_disabled: '源已停用',
  source_changed: '源已变更',
  budget: '预算跳过',
  cancelled: '已取消',
  guard: '安全阀暂停',
  unknown: '未校验',
};

const PROBE_LABEL: Record<string, string> = {
  reachability: '连通',
  playlist: '清单',
  speed: '测速',
  full: '完整',
};

const OVERRIDE_LABEL: Record<string, string> = {
  enabled: '开关',
  min_speed_mib: '最低速率',
  fallback_speed_mib: '兜底速率',
  max_delay_ms: '最大延迟',
  min_resolution: '最低分辨率',
  max_resolution: '最高分辨率',
  resolution_speed_map: '分辨率速率表',
  supply: '兜底',
  action: '动作',
  retain: '保留',
  host_cache: 'Host 缓存',
  stale_keep_secs: '失败宽限',
};

function timeText(epoch: number): string {
  if (!Number.isFinite(epoch) || epoch <= 0) return '—';
  return new Date(epoch * 1000).toLocaleString('zh-CN', { hour12: false });
}

function bytesText(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B';
  if (bytes >= 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`;
  if (bytes >= 1024) return `${(bytes / 1024).toFixed(1)} KiB`;
  return `${bytes} B`;
}

function relativeText(epoch: number): string {
  if (!Number.isFinite(epoch) || epoch <= 0) return '—';
  const diff = Math.max(0, Math.floor(Date.now() / 1000) - epoch);
  if (diff < 60) return '刚刚';
  if (diff < 3600) return `${Math.floor(diff / 60)} 分钟前`;
  if (diff < 86_400) return `${Math.floor(diff / 3600)} 小时前`;
  return `${Math.floor(diff / 86_400)} 天前`;
}

function resolutionText(value: string): string {
  const height = value.split('x')[1];
  return height ? `${height}p` : '—';
}

type QualityRoute = {
  tab: Tab;
  gid: string;
  cid: string;
  source_id: string;
  action: string;
  reason: string;
  q: string;
};

/** 读取 `#/quality/<tab>?...`。 */
function qualityRoute(hash = window.location.hash): QualityRoute {
  const raw = hash.replace(/^#\/?/, '');
  const [path, query = ''] = raw.split('?');
  const parts = path.split('/');
  const tab = TABS.some(([id]) => id === parts[1]) ? (parts[1] as Tab) : 'overview';
  const params = new URLSearchParams(query);
  return {
    tab,
    gid: params.get('gid') ?? '',
    cid: params.get('cid') ?? '',
    source_id: params.get('source_id') ?? '',
    action: params.get('action') ?? '',
    reason: params.get('reason') ?? '',
    q: params.get('q') ?? '',
  };
}

function broadcastQualityChanged() {
  window.dispatchEvent(new Event('tv-buddy:quality-changed'));
}

function getPath(root: Cfg, path: string): unknown {
  return path.split('.').reduce<unknown>((acc, key) => {
    if (acc && typeof acc === 'object') return (acc as Cfg)[key];
    return undefined;
  }, root);
}

function setPath(root: Cfg, path: string, value: unknown): Cfg {
  const [head, ...rest] = path.split('.');
  if (rest.length === 0) return { ...root, [head]: value };
  const child = (root[head] ?? {}) as Cfg;
  return { ...root, [head]: setPath(child, rest.join('.'), value) };
}

function asNumber(root: Cfg, path: string): number {
  const value = getPath(root, path);
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

function asText(root: Cfg, path: string): string {
  const value = getPath(root, path);
  return value === null || value === undefined ? '' : String(value);
}

function asBool(root: Cfg, path: string): boolean {
  return getPath(root, path) === true;
}

type SpeedRow = { resolution: string; speed: string };

function readSpeedRows(root: Cfg): SpeedRow[] {
  const value = getPath(root, 'quality.rules.resolution_speed_map');
  if (!value || typeof value !== 'object' || Array.isArray(value)) return [];
  return Object.entries(value as Cfg).map(([resolution, speed]) => ({
    resolution,
    speed: speed === null || speed === undefined ? '' : String(speed),
  }));
}

function speedRowsToMap(rows: SpeedRow[]): Record<string, number> {
  const map: Record<string, number> = {};
  for (const row of rows) {
    const resolution = row.resolution.trim();
    const speed = Number(row.speed);
    if (resolution !== '' && Number.isFinite(speed) && speed >= 0) {
      map[resolution] = speed;
    }
  }
  return map;
}

/** 数字输入。编辑期间保留原始文本，避免 0、空值与小数点被默认值覆盖。 */
function NumberInput({
  value,
  min,
  max,
  step,
  onChange,
}: {
  value: number;
  min?: number;
  max?: number;
  step?: number;
  onChange: (value: number) => void;
}) {
  const [text, setText] = useState(String(value));
  const [focused, setFocused] = useState(false);
  useEffect(() => {
    if (!focused) setText(String(value));
  }, [focused, value]);
  const commit = (raw: string) => {
    const parsed = Number(raw);
    if (raw.trim() === '' || !Number.isFinite(parsed)) return;
    let next = parsed;
    if (min !== undefined && next < min) next = min;
    if (max !== undefined && next > max) next = max;
    onChange(next);
  };
  return (
    <input
      type="number"
      min={min}
      max={max}
      step={step}
      value={text}
      onFocus={() => setFocused(true)}
      onChange={(event) => {
        setText(event.target.value);
        commit(event.target.value);
      }}
      onBlur={() => {
        setFocused(false);
        const parsed = Number(text);
        if (text.trim() === '' || !Number.isFinite(parsed)) setText(String(value));
      }}
    />
  );
}

function overrideFields(value: unknown): string {
  if (!value || typeof value !== 'object') return '';
  return Object.entries(value as Cfg)
    .filter(([, item]) => item !== null && item !== undefined)
    .map(([key]) => OVERRIDE_LABEL[key] ?? key)
    .join('、');
}

/** 概览标签。 */
function OverviewTab({
  status,
  busy,
  onRun,
  onCancel,
}: {
  status: QualityStatus | null;
  busy: boolean;
  onRun: () => void;
  onCancel: () => void;
}) {
  if (!status) return <Empty text="正在读取质量状态…" icon="activity" />;
  const run = status.run;
  const scheduleText =
    status.schedule.mode === 'cron'
      ? `cron：${status.schedule.cron}`
      : `每 ${status.schedule.interval_hours} 小时`;
  return (
    <>
      <div className="stats">
        <div className="stat">
          <div className="k">
            <Icon name="activity" size={13} />
            质量校验
          </div>
          <div className="v">
            <Badge tone={status.enabled ? 'ok' : 'muted'}>
              {status.enabled ? '已开启' : '已关闭'}
            </Badge>
          </div>
          <div className="hint">
            {scheduleText}；下次 {timeText(status.schedule.next_run_at)}
          </div>
        </div>
        <div className="stat">
          <div className="k">当前准入</div>
          <div className="v">{status.counts.admitted}</div>
          <div className="hint">结果表中状态为准入的频道数</div>
        </div>
        <div className="stat">
          <div className="k">近 7 天移出</div>
          <div className="v">{status.counts.removed_7d}</div>
          <div className="hint">共 {status.counts.sources} 个同步源参与门控</div>
        </div>
      </div>
      {status.guard.tripped || status.source_errors.length > 0 || status.counts.probe_errors_7d > 0 ? (
        <div className="card">
          <div className="card-head">
            <div className="card-head-text">
              <h2>警告</h2>
              <span className="muted">按影响排序</span>
            </div>
          </div>
          <ul>
            {status.guard.tripped ? (
              <li>
                <Badge tone="danger">安全阀</Badge> 上一轮移出达到阈值，成员变更已暂停。
                <span className="hint">
                  阈值：{(status.guard.max_remove_ratio * 100).toFixed(0)}%，至少{' '}
                  {status.guard.min_remove_count} 个
                </span>
              </li>
            ) : null}
            {status.source_errors.map((source) => (
              <li key={source.id}>
                <Badge tone="warn">源抓取</Badge> {source.name || source.id}：
                {source.error}
              </li>
            ))}
            {status.counts.probe_errors_7d > 0 ? (
              <li>
                <Badge tone="muted">探测错误</Badge> 近 7 天有{' '}
                {status.counts.probe_errors_7d} 条探测错误，请检查解析脚本或探测地址。
              </li>
            ) : null}
          </ul>
        </div>
      ) : null}
      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>最近任务</h2>
            <span className="muted">
              {run ? `${run.trigger} · ${run.phase}` : '还没有运行记录'}
            </span>
          </div>
          <div className="card-actions">
            <button
              className="ghost"
              onClick={onRun}
              disabled={busy || status.running}
              title="按当前已保存的规则校验全部频道"
            >
              {busy ? '已排入…' : '立即校验'}
            </button>
            {status.running ? (
              <button className="ghost" onClick={onCancel} disabled={busy}>
                取消任务
              </button>
            ) : null}
          </div>
        </div>
        {run ? (
          <dl>
            <dt>任务</dt>
            <dd className="mono">{run.run_id}</dd>
            <dt>开始</dt>
            <dd>{timeText(run.started_at)}</dd>
            <dt>进度</dt>
            <dd>
              {run.tested} / {run.total}，准入 {run.admitted}，移出 {run.removed}，拒绝{' '}
              {run.rejected}，跳过 {run.skipped}
            </dd>
            <dt>流量</dt>
            <dd>{bytesText(run.bytes)}</dd>
            <dt>状态</dt>
            <dd>
              {run.finished ? '已完成' : '进行中'}
              {run.cancelled ? '；已取消' : ''}
              {run.paused_by_guard ? '；安全阀已暂停' : ''}
              {run.measure_only ? '；仅测量（质量已关闭）' : ''}
            </dd>
            {run.error ? (
              <>
                <dt>错误</dt>
                <dd>{run.error}</dd>
              </>
            ) : null}
          </dl>
        ) : (
          <Empty text="尚未运行质量校验" icon="activity" />
        )}
      </div>
      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>安全阀</h2>
            <span className="muted">阻止一次运行移出过多频道</span>
          </div>
        </div>
        <dl>
          <dt>开关</dt>
          <dd>
            <Badge tone={status.guard.enabled ? 'ok' : 'muted'}>
              {status.guard.enabled ? '已开启' : '已关闭'}
            </Badge>
            {status.guard.tripped ? <Badge tone="danger">已触发</Badge> : null}
          </dd>
          <dt>触发条件</dt>
          <dd>
            移出比例超过 {(status.guard.max_remove_ratio * 100).toFixed(0)}%，且至少{' '}
            {status.guard.min_remove_count} 个
          </dd>
        </dl>
      </div>
    </>
  );
}

/** 规则标签。 */
function RulesTab({
  status,
  onSaved,
}: {
  status: QualityStatus | null;
  onSaved: () => void;
}) {
  const { node, ok, err } = useToast();
  const [draft, setDraft] = useState<Cfg | null>(null);
  const [busy, setBusy] = useState('');
  const [preview, setPreview] = useState<number[]>([]);
  const [previewError, setPreviewError] = useState('');
  const [speedRows, setSpeedRows] = useState<SpeedRow[]>([]);

  const load = useCallback(async () => {
    try {
      const config = await api<Cfg>('GET', '/config');
      setDraft(config);
      setSpeedRows(readSpeedRows(config));
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const update = (path: string, value: unknown) => {
    setDraft((prev) => (prev ? setPath(prev, path, value) : prev));
  };

  const commitSpeedRows = (next: SpeedRow[]) => {
    setSpeedRows(next);
    update('quality.rules.resolution_speed_map', speedRowsToMap(next));
  };

  const previewSchedule = async () => {
    if (!draft) return;
    setBusy('preview');
    setPreviewError('');
    try {
      const mode = asText(draft, 'quality.schedule') === 'cron' ? 'cron' : 'interval';
      const result = await previewQualitySchedule({
        mode,
        cron: asText(draft, 'quality.cron'),
        interval_hours: asNumber(draft, 'quality.interval_hours'),
      });
      setPreview(result.times ?? []);
    } catch (error) {
      setPreview([]);
      setPreviewError(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const save = async () => {
    if (!draft) return;
    setBusy('save');
    try {
      const revision = await putConfig(draft);
      setDraft({ ...draft, revision });
      ok('质量规则已保存');
      onSaved();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const overrideRows = useMemo(() => {
    if (!draft) return [];
    const rows: { layer: string; scope: string; fields: string }[] = [];
    const sources = Array.isArray(getPath(draft, 'playlist.sources'))
      ? (getPath(draft, 'playlist.sources') as Cfg[])
      : [];
    for (const source of sources) {
      const fields = overrideFields(source.quality);
      if (fields) {
        rows.push({
          layer: '源',
          scope: String(source.name || source.id || '未命名'),
          fields,
        });
      }
    }
    const groups = Array.isArray(draft.groups) ? (draft.groups as Cfg[]) : [];
    for (const group of groups) {
      const fields = overrideFields(group.quality);
      if (fields) {
        rows.push({
          layer: '分组',
          scope: String(group.name || group.id || '未命名'),
          fields,
        });
      }
      const channels = Array.isArray(group.channels) ? (group.channels as Cfg[]) : [];
      for (const channel of channels) {
        const channelFields = overrideFields(channel.quality);
        if (channelFields) {
          rows.push({
            layer: '频道',
            scope: `${String(group.name || group.id)} / ${String(channel.name || channel.id)}`,
            fields: channelFields,
          });
        }
      }
    }
    return rows;
  }, [draft]);

  if (!draft) return <Empty text="正在读取配置…" icon="settings" />;

  const scheduleMode = asText(draft, 'quality.schedule') === 'cron' ? 'cron' : 'interval';
  const configuredProbe = asText(draft, 'quality.probe_mode');
  const ffprobeEnabled = asBool(draft, 'quality.ffprobe');
  const speedRule = asNumber(draft, 'quality.rules.min_speed_mib') > 0;
  const resolutionRule =
    asText(draft, 'quality.rules.min_resolution').trim() !== '' ||
    asText(draft, 'quality.rules.max_resolution').trim() !== '';
  const actualProbe = speedRule
    ? 'speed'
    : resolutionRule && configuredProbe === 'reachability'
      ? 'playlist'
      : configuredProbe;
  const probeHint =
    configuredProbe === 'full' && !ffprobeEnabled
      ? '实际档位：测速（ffprobe 未启用）'
      : actualProbe === configuredProbe
        ? undefined
        : `实际档位：${PROBE_LABEL[actualProbe] ?? actualProbe}（规则自动提升）`;
  return (
    <>
      {node}
      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>运行参数</h2>
            <span className="muted">保存后立即生效</span>
          </div>
          <div className="card-actions">
            <button className="ghost" onClick={() => void load()} disabled={busy !== ''}>
              重新读取
            </button>
            <button onClick={() => void save()} disabled={busy !== ''}>
              {busy === 'save' ? '保存中…' : '保存规则'}
            </button>
          </div>
        </div>
        <div className="fields">
          <Field label="质量校验" tip="关闭后不再自动准入或移出；手动运行只测量。">
            <Switch
              checked={asBool(draft, 'quality.enabled')}
              onChange={(next) => update('quality.enabled', next)}
              label="质量校验"
            />
          </Field>
          <Field label="调度方式" tip="按固定小时数或 cron 表达式运行。">
            <Select
              ariaLabel="调度方式"
              value={scheduleMode}
              compact
              options={[
                { value: 'interval', label: '按小时', desc: '每 N 小时运行一次。' },
                { value: 'cron', label: 'cron', desc: '按 5 段 cron 表达式运行。' },
              ]}
              onChange={(next) => update('quality.schedule', next)}
            />
          </Field>
          <Field label="间隔小时" hint="1–8760">
            <NumberInput
              min={1}
              max={8760}
              value={asNumber(draft, 'quality.interval_hours')}
              onChange={(value) => update('quality.interval_hours', value)}
            />
          </Field>
          <Field label="cron 表达式" hint="分 时 日 月 周，按服务器时区">
            <input
              value={asText(draft, 'quality.cron')}
              onChange={(event) => update('quality.cron', event.target.value)}
              placeholder="0 */6 * * *"
            />
          </Field>
          <Field label="下次运行预览">
            <button
              className="ghost"
              onClick={() => void previewSchedule()}
              disabled={busy !== ''}
            >
              {busy === 'preview' ? '计算中…' : '预览未来 3 次'}
            </button>
          </Field>
          <Field label="探测档位" tip="档位越高，耗时与流量越多。" hint={probeHint}>
            <Select
              ariaLabel="探测档位"
              value={asText(draft, 'quality.probe_mode')}
              compact
              options={[
                { value: 'reachability', label: '连通', desc: '只检查能否建立连接。' },
                { value: 'playlist', label: '清单', desc: '读取清单并请求首个切片。' },
                { value: 'speed', label: '测速', desc: '下载样本并测量速率。' },
                { value: 'full', label: '完整', desc: '测速，并按需识别分辨率。' },
              ]}
              onChange={(next) => update('quality.probe_mode', next)}
            />
          </Field>
          <Field label="校验超时" hint="秒">
            <NumberInput
              min={1}
              value={asNumber(draft, 'quality.timeout_secs')}
              onChange={(value) => update('quality.timeout_secs', value)}
            />
          </Field>
          <Field label="测速超时" hint="秒">
            <NumberInput
              min={1}
              value={asNumber(draft, 'quality.speed_timeout_secs')}
              onChange={(value) => update('quality.speed_timeout_secs', value)}
            />
          </Field>
          <Field label="校验并发" hint="1–32">
            <NumberInput
              min={1}
              max={32}
              value={asNumber(draft, 'quality.concurrency')}
              onChange={(value) => update('quality.concurrency', value)}
            />
          </Field>
          <Field label="单轮流量预算" hint="MB，0 = 不限制">
            <NumberInput
              min={0}
              value={asNumber(draft, 'quality.budget_mb_per_run')}
              onChange={(value) => update('quality.budget_mb_per_run', value)}
            />
          </Field>
          <Field label="单轮频道上限" hint="0 = 不限制">
            <NumberInput
              min={0}
              value={asNumber(draft, 'quality.max_channels_per_run')}
              onChange={(value) => update('quality.max_channels_per_run', value)}
            />
          </Field>
          <Field label="单轮时限" hint="秒，门控与校验共用；0 = 不限制">
            <NumberInput
              min={0}
              value={asNumber(draft, 'quality.gate_time_budget_secs')}
              onChange={(value) => update('quality.gate_time_budget_secs', value)}
            />
          </Field>
          <Field label="测速采样时长" hint="秒">
            <NumberInput
              min={0.1}
              step={0.5}
              value={asNumber(draft, 'quality.sample_secs')}
              onChange={(value) => update('quality.sample_secs', value)}
            />
          </Field>
        </div>
        {previewError ? <div className="hint">{previewError}</div> : null}
        {preview.length > 0 ? (
          <div className="hint">未来运行：{preview.map(timeText).join('；')}</div>
        ) : null}
      </div>
      <div className="card quality-rules-card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>判定阈值</h2>
            <span className="muted">频道、分组、来源的覆盖优先于这里的全局值</span>
          </div>
        </div>
        <div className="quality-rules-grid">
          <h3 className="quality-section-title">速率与延迟</h3>
          <Field label="最低速率" hint="MiB/s，0 = 不限制">
            <NumberInput
              min={0}
              step={0.1}
              value={asNumber(draft, 'quality.rules.min_speed_mib')}
              onChange={(value) => update('quality.rules.min_speed_mib', value)}
            />
          </Field>
          <Field label="兜底速率" hint="MiB/s，无法识别分辨率，或速率表未命中时使用；0 = 用最低速率">
            <NumberInput
              min={0}
              step={0.1}
              value={asNumber(draft, 'quality.rules.fallback_speed_mib')}
              onChange={(value) => update('quality.rules.fallback_speed_mib', value)}
            />
          </Field>
          <Field label="最大延迟" hint="毫秒，0 = 不限制">
            <NumberInput
              min={0}
              value={asNumber(draft, 'quality.rules.max_delay_ms')}
              onChange={(value) => update('quality.rules.max_delay_ms', value)}
            />
          </Field>
          <h3 className="quality-section-title">分辨率</h3>
          <Field label="最低分辨率" hint="如 1280x720，留空不限制">
            <input
              value={asText(draft, 'quality.rules.min_resolution')}
              onChange={(event) => update('quality.rules.min_resolution', event.target.value)}
              placeholder="1280x720"
            />
          </Field>
          <Field label="最高分辨率" hint="如 3840x2160，留空不限制">
            <input
              value={asText(draft, 'quality.rules.max_resolution')}
              onChange={(event) => update('quality.rules.max_resolution', event.target.value)}
              placeholder="3840x2160"
            />
          </Field>
          <Field label="Host 缓存" tip="复用同一 Host 的成功测速，减少重复下载。">
            <Switch
              checked={asBool(draft, 'quality.rules.host_cache')}
              onChange={(next) => update('quality.rules.host_cache', next)}
              label="Host 缓存"
            />
          </Field>
          <Field
            label="分辨率速率要求"
            hint="按实际分辨率取不超过它的最高档；未命中时用兜底速率；空行不保存"
            className="quality-field-wide"
          >
            <table className="quality-speed-table">
              <thead>
                <tr>
                  <th>分辨率</th>
                  <th>最低速率 (MiB/s)</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {speedRows.map((row, index) => (
                  <tr key={index}>
                    <td data-label="分辨率">
                      <input
                        aria-label={`第 ${index + 1} 行分辨率`}
                        value={row.resolution}
                        placeholder="1280x720"
                        onChange={(event) => {
                          const next = speedRows.map((item, itemIndex) =>
                            itemIndex === index
                              ? { ...item, resolution: event.target.value }
                              : item,
                          );
                          commitSpeedRows(next);
                        }}
                      />
                    </td>
                    <td data-label="最低速率 (MiB/s)">
                      <input
                        aria-label={`第 ${index + 1} 行最低速率`}
                        type="number"
                        min={0}
                        step={0.1}
                        value={row.speed}
                        placeholder="MiB/s"
                        onChange={(event) => {
                          const next = speedRows.map((item, itemIndex) =>
                            itemIndex === index
                              ? { ...item, speed: event.target.value }
                              : item,
                          );
                          commitSpeedRows(next);
                        }}
                      />
                    </td>
                    <td>
                      <button
                        type="button"
                        className="ghost"
                        onClick={() =>
                          commitSpeedRows(
                            speedRows.filter((_row, itemIndex) => itemIndex !== index),
                          )
                        }
                      >
                        删除
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <button
              type="button"
              className="ghost"
              onClick={() => commitSpeedRows([...speedRows, { resolution: '', speed: '1' }])}
            >
              添加分辨率
            </button>
          </Field>
          <h3 className="quality-section-title">其他</h3>
          <Field label="手工下线宽限" hint="秒，0 = 立即下线">
            <NumberInput
              min={0}
              value={asNumber(draft, 'quality.stale_keep_secs')}
              onChange={(value) => update('quality.stale_keep_secs', value)}
            />
          </Field>
        </div>
      </div>
      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>分辨率探测</h2>
            <span className="muted">完整档读取切片开头，用 ffprobe 识别分辨率</span>
          </div>
        </div>
        <div className="fields">
          <Field label="启用分辨率识别" tip="只在完整档生效。未安装 ffprobe 时使用兜底速率。">
            <Switch
              checked={asBool(draft, 'quality.ffprobe')}
              onChange={(next) => update('quality.ffprobe', next)}
              label="启用分辨率识别"
            />
          </Field>
          <Field label="单次超时" hint="秒，1–30">
            <NumberInput
              min={1}
              max={30}
              value={asNumber(draft, 'quality.ffprobe_timeout_secs')}
              onChange={(value) => update('quality.ffprobe_timeout_secs', value)}
            />
          </Field>
          <Field label="并发" hint="1–4">
            <NumberInput
              min={1}
              max={4}
              value={asNumber(draft, 'quality.probe_concurrency')}
              onChange={(value) => update('quality.probe_concurrency', value)}
            />
          </Field>
          <Field label="单轮上限" hint="0 = 不限制">
            <NumberInput
              min={0}
              value={asNumber(draft, 'quality.max_ffprobe_per_run')}
              onChange={(value) => update('quality.max_ffprobe_per_run', value)}
            />
          </Field>
          <Field label="最低测速门槛" hint="MiB/s，0 = 不限制">
            <NumberInput
              min={0}
              step={0.1}
              value={asNumber(draft, 'quality.ffprobe_min_speed_mib')}
              onChange={(value) => update('quality.ffprobe_min_speed_mib', value)}
            />
          </Field>
          <Field label="采样大小" hint="KiB，读取切片开头。64–2048">
            <NumberInput
              min={64}
              max={2048}
              value={asNumber(draft, 'quality.ffprobe_sample_kb')}
              onChange={(value) => update('quality.ffprobe_sample_kb', value)}
            />
          </Field>
        </div>
        {status && !status.ffprobe.installed ? (
          <div className="hint">未安装 ffprobe。分辨率识别不可用，规则会使用兜底速率。</div>
        ) : null}
        {status && status.ffprobe.installed && !status.ffprobe.enabled ? (
          <div className="hint">开启后，完整档会读取切片开头识别分辨率。</div>
        ) : null}
      </div>
      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>安全阀</h2>
            <span className="muted">一次运行移出过多时暂停成员变更</span>
          </div>
        </div>
        <div className="fields">
          <Field label="启用安全阀">
            <Switch
              checked={asBool(draft, 'quality.guard_enabled')}
              onChange={(next) => update('quality.guard_enabled', next)}
              label="启用安全阀"
            />
          </Field>
          <Field label="最大移出比例" hint="0 到 1，如 0.5 表示 50%">
            <NumberInput
              min={0.01}
              max={1}
              step={0.01}
              value={asNumber(draft, 'quality.max_remove_ratio')}
              onChange={(value) => update('quality.max_remove_ratio', value)}
            />
          </Field>
          <Field label="最小移出个数" hint="移出数量低于该值不触发">
            <NumberInput
              min={1}
              value={asNumber(draft, 'quality.min_remove_count')}
              onChange={(value) => update('quality.min_remove_count', value)}
            />
          </Field>
          <Field label="结果保留天数" hint="0 = 不自动清理">
            <NumberInput
              min={0}
              max={3650}
              value={asNumber(draft, 'quality.result_retention_days')}
              onChange={(value) => update('quality.result_retention_days', value)}
            />
          </Field>
        </div>
      </div>
      <Card title="覆盖表" note="只列出配置了覆盖的层级；字段顺序为 频道 > 分组 > 源 > 全局">
        {overrideRows.length === 0 ? (
          <Empty text="没有来源、分组或频道覆盖" icon="settings" />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>层级</th>
                <th>范围</th>
                <th>覆盖字段</th>
              </tr>
            </thead>
            <tbody>
              {overrideRows.map((row) => (
                <tr key={`${row.layer}-${row.scope}`}>
                  <td>{row.layer}</td>
                  <td>{row.scope}</td>
                  <td>{row.fields}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </Card>
    </>
  );
}

/** 结果标签。 */
function ResultsTab({
  running,
  initialGid,
  initialCid,
  initialSourceId,
  initialAction,
  initialReason,
  initialQuery,
}: {
  running: boolean;
  initialGid: string;
  initialCid: string;
  initialSourceId: string;
  initialAction: string;
  initialReason: string;
  initialQuery: string;
}) {
  const { node, ok, err } = useToast();
  const [filters, setFilters] = useState({
    source_id: initialSourceId,
    gid: initialGid,
    cid: initialCid,
    action: initialAction,
    reason: initialReason,
    q: initialQuery,
  });
  const [limit, setLimit] = useState(100);
  const [offset, setOffset] = useState(0);
  const [data, setData] = useState<{ total: number; items: QualityResult[] } | null>(null);
  const [groups, setGroups] = useState<{ id: string; name: string }[]>([]);
  const [sources, setSources] = useState<{ id: string; name: string }[]>([]);
  const [detail, setDetail] = useState<QualityResult | null>(null);
  const [rerun, setRerun] = useState<QualityResult | null>(null);
  const requestSeq = useRef(0);

  useEffect(() => {
    setFilters({
      source_id: initialSourceId,
      gid: initialGid,
      cid: initialCid,
      action: initialAction,
      reason: initialReason,
      q: initialQuery,
    });
    setOffset(0);
  }, [
    initialGid,
    initialCid,
    initialSourceId,
    initialAction,
    initialReason,
    initialQuery,
  ]);

  // 筛选写入地址栏，刷新、复制链接与返回时都能恢复。
  useEffect(() => {
    const params = new URLSearchParams();
    if (filters.source_id) params.set('source_id', filters.source_id);
    if (filters.gid) params.set('gid', filters.gid);
    if (filters.cid) params.set('cid', filters.cid);
    if (filters.action) params.set('action', filters.action);
    if (filters.reason) params.set('reason', filters.reason);
    if (filters.q.trim()) params.set('q', filters.q.trim());
    const query = params.toString();
    window.history.replaceState(
      null,
      '',
      `#/quality/results${query ? `?${query}` : ''}`,
    );
  }, [filters]);

  useEffect(() => {
    void api<Cfg>('GET', '/config')
      .then((config) => {
        const rawGroups = Array.isArray(config.groups) ? (config.groups as Cfg[]) : [];
        setGroups(
          rawGroups.map((group) => ({
            id: String(group.id ?? ''),
            name: String(group.name || group.id || ''),
          })),
        );
        const playlist = (config.playlist ?? {}) as Cfg;
        const rawSources = Array.isArray(playlist.sources)
          ? (playlist.sources as Cfg[])
          : [];
        setSources(
          rawSources
            .filter((source) => source.mode === 'sync')
            .map((source) => ({
              id: String(source.id ?? ''),
              name: String(source.name || source.id || ''),
            })),
        );
      })
      .catch(() => {
        // 过滤选项取不到不影响结果列表。
      });
  }, []);

  const load = useCallback(async () => {
    const seq = ++requestSeq.current;
    try {
      const body = await fetchQualityResults({
        ...filters,
        limit,
        offset,
      });
      if (seq === requestSeq.current) setData(body);
    } catch (error) {
      if (seq === requestSeq.current) {
        err(error instanceof Error ? error.message : String(error));
      }
    }
  }, [err, filters, limit, offset]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => void load(), 2000);
    return () => window.clearInterval(timer);
  }, [load, running]);

  const sourceName = (id: string) =>
    sources.find((source) => source.id === id)?.name ?? (id || '手工');

  const filtered =
    filters.source_id !== '' ||
    filters.gid !== '' ||
    filters.cid !== '' ||
    filters.action !== '' ||
    filters.reason !== '' ||
    filters.q.trim() !== '';

  const setFilter = (key: keyof typeof filters, value: string) => {
    setFilters((prev) => ({ ...prev, [key]: value }));
    setOffset(0);
  };

  const doRerun = async () => {
    if (!rerun) return;
    try {
      const result = await runQuality('channel', `${rerun.gid}/${rerun.cid}`);
      ok(result.measure_only ? '质量已关闭，本次只测量' : '已排入重新校验');
      setRerun(null);
      await load();
      broadcastQualityChanged();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const total = data?.total ?? 0;
  const items = data?.items ?? [];
  const pageEnd = Math.min(offset + limit, total);

  return (
    <>
      {node}
      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>结果</h2>
            <span className="muted">共 {total} 条{filters.cid ? `，频道 ${filters.cid}` : ''}</span>
          </div>
          <div className="card-actions">
            <button className="ghost" onClick={() => void load()}>
              刷新
            </button>
          </div>
        </div>
        <div className="fields">
          <Field label="来源">
            <Select
              ariaLabel="来源筛选"
              value={filters.source_id}
              compact
              options={[
                { value: '', label: '全部来源' },
                ...sources.map((source) => ({ value: source.id, label: source.name })),
              ]}
              onChange={(next) => setFilter('source_id', next)}
            />
          </Field>
          <Field label="分组">
            <Select
              ariaLabel="分组筛选"
              value={filters.gid}
              compact
              options={[
                { value: '', label: '全部分组' },
                ...groups.map((group) => ({ value: group.id, label: group.name })),
              ]}
              onChange={(next) => setFilter('gid', next)}
            />
          </Field>
          <Field label="动作">
            <Select
              ariaLabel="动作筛选"
              value={filters.action}
              compact
              options={[
                { value: '', label: '全部动作' },
                { value: 'admitted', label: '已准入' },
                { value: 'rejected', label: '未准入' },
                { value: 'removed', label: '已移出' },
                { value: 'hidden', label: '已下线' },
                { value: 'kept', label: '已保留' },
                { value: 'source_removed', label: '源已移除' },
                { value: 'held', label: '暂停准入' },
                { value: 'removal_paused', label: '暂停移出' },
                { value: 'skipped', label: '跳过' },
              ]}
              onChange={(next) => setFilter('action', next)}
            />
          </Field>
          <Field label="原因">
            <Select
              ariaLabel="原因筛选"
              value={filters.reason}
              compact
              options={[
                { value: '', label: '全部原因' },
                { value: 'pass', label: '合格' },
                { value: 'unreachable', label: '不可达' },
                { value: 'speed_low', label: '速率低' },
                { value: 'resolution_low', label: '分辨率低' },
                { value: 'resolution_high', label: '分辨率高' },
                { value: 'delay_high', label: '延迟高' },
                { value: 'probe_error', label: '探测错误' },
                { value: 'guard', label: '安全阀暂停' },
                { value: 'source_removed', label: '源已移除' },
              ]}
              onChange={(next) => setFilter('reason', next)}
            />
          </Field>
          <Field label="搜索">
            <input
              value={filters.q}
              placeholder="频道名或 id"
              onChange={(event) => setFilter('q', event.target.value)}
            />
          </Field>
          {filters.cid ? (
            <Field label="当前频道">
              <button
                className="ghost"
                onClick={() => {
                  setFilter('gid', '');
                  setFilter('cid', '');
                }}
              >
                清除频道筛选
              </button>
            </Field>
          ) : null}
        </div>
      </div>
      <div className="card">
        {items.length === 0 ? (
          <Empty
            text={filtered ? '没有匹配结果' : '没有结果记录'}
            icon="activity"
          />
        ) : (
          <>
            <table className="table">
              <thead>
                <tr>
                  <th>频道</th>
                  <th>来源</th>
                  <th>目标分组</th>
                  <th>动作</th>
                  <th>原因</th>
                  <th>延迟</th>
                  <th>速率</th>
                  <th>分辨率</th>
                  <th>最近校验</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {items.map((item) => (
                  <tr key={item.key}>
                    <td>
                      {item.name || item.cid}
                      <span className="cid">[{item.cid}]</span>
                    </td>
                    <td>{item.source_id ? sourceName(item.source_id) : '手工'}</td>
                    <td>{item.target_group || '—'}</td>
                    <td>
                      <Badge tone={resultActionTone(item)}>{resultActionText(item)}</Badge>
                    </td>
                    <td>{REASON_LABEL[item.reason] ?? item.reason}</td>
                    <td>{item.delay_ms === null ? '—' : `${item.delay_ms} ms`}</td>
                    <td>
                      {item.speed_mib === null ? '—' : `${item.speed_mib.toFixed(1)} MiB/s`}
                    </td>
                    <td>{resolutionText(item.resolution)}</td>
                    <td title={timeText(item.checked_at)}>{relativeText(item.checked_at)}</td>
                    <td>
                      <button className="chip" onClick={() => setDetail(item)}>
                        详情
                      </button>
                      {item.action === 'admitted' ? (
                        <button className="chip" onClick={() => setRerun(item)}>
                          重新校验
                        </button>
                      ) : null}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
            <div className="card-head">
              <span className="muted">
                第 {total === 0 ? 0 : offset + 1}–{pageEnd} 条，共 {total} 条
              </span>
              <div className="card-actions">
                <Select
                  ariaLabel="每页数量"
                  value={String(limit)}
                  compact
                  options={[
                    { value: '50', label: '50 条' },
                    { value: '100', label: '100 条' },
                    { value: '200', label: '200 条' },
                  ]}
                  onChange={(next) => {
                    setLimit(Number(next));
                    setOffset(0);
                  }}
                />
                <button
                  className="ghost"
                  disabled={offset === 0}
                  onClick={() => setOffset(Math.max(0, offset - limit))}
                >
                  上一页
                </button>
                <button
                  className="ghost"
                  disabled={pageEnd >= total}
                  onClick={() => setOffset(offset + limit)}
                >
                  下一页
                </button>
              </div>
            </div>
          </>
        )}
      </div>
      {detail ? (
        <Modal title="质量结果详情" onClose={() => setDetail(null)}>
          <dl>
            <dt>频道</dt>
            <dd>
              {detail.name || detail.cid} <span className="cid">[{detail.cid}]</span>
            </dd>
            <dt>来源</dt>
            <dd>{detail.source_id ? sourceName(detail.source_id) : '手工'}</dd>
            <dt>目标分组</dt>
            <dd>{detail.target_group || '—'}</dd>
            <dt>动作</dt>
            <dd>{resultActionText(detail)}</dd>
            <dt>原因</dt>
            <dd>{REASON_LABEL[detail.reason] ?? detail.reason}</dd>
            <dt>最近校验</dt>
            <dd>{timeText(detail.checked_at)}</dd>
            <dt>首次失败</dt>
            <dd>{timeText(detail.first_fail_at ?? 0)}</dd>
            <dt>延迟</dt>
            <dd>{detail.delay_ms === null ? '—' : `${detail.delay_ms} ms`}</dd>
            <dt>速率</dt>
            <dd>
              {detail.speed_mib === null ? '—' : `${detail.speed_mib.toFixed(2)} MiB/s`}
            </dd>
            <dt>分辨率</dt>
            <dd>{detail.resolution || '—'}</dd>
            <dt>HTTP 状态</dt>
            <dd>{detail.http_status ?? '—'}</dd>
            <dt>地址哈希</dt>
            <dd className="mono">{detail.url_hash || '—'}</dd>
          </dl>
          {detail.action === 'removed' || detail.action === 'source_removed' ? (
            <p className="hint">需要重新同步源才能再次准入。</p>
          ) : null}
        </Modal>
      ) : null}
      {rerun ? (
        <Confirm
          title="重新校验这个频道？"
          text={`将校验「${rerun.name || rerun.cid}」。失败会立即从分组移出。`}
          confirmText="开始校验"
          onCancel={() => setRerun(null)}
          onConfirm={() => void doRerun()}
        />
      ) : null}
    </>
  );
}

/** 来源标签。 */
function SourcesTab({ onChanged }: { onChanged: () => void }) {
  const { node, ok, err } = useToast();
  const [items, setItems] = useState<QualitySource[] | null>(null);
  const [configSources, setConfigSources] = useState<PlaylistSourceConfig[]>([]);
  const [busy, setBusy] = useState('');
  const [gateOff, setGateOff] = useState<QualitySource | null>(null);
  const [disable, setDisable] = useState<{
    source: QualitySource;
    keep: boolean;
    remember: boolean;
    confirm: boolean;
  } | null>(null);
  const [removing, setRemoving] = useState<{
    source: QualitySource;
    keep: boolean;
    confirm: boolean;
  } | null>(null);

  const load = useCallback(async () => {
    try {
      const [quality, playlist] = await Promise.all([
        fetchQualitySources(),
        fetchPlaylistSources(),
      ]);
      setItems(quality.items ?? []);
      setConfigSources(playlist.sources ?? []);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const change = async (message: string) => {
    ok(message);
    broadcastQualityChanged();
    onChanged();
    await load();
  };

  const toggleGate = async (item: QualitySource, next: boolean) => {
    const config = configSources.find((source) => source.id === item.id);
    if (!config) {
      err('找不到源配置');
      return;
    }
    setBusy(`gate-${item.id}`);
    try {
      const quality = { ...(config.quality ?? {}), enabled: next };
      await updatePlaylistSource(item.id, { ...config, quality });
      await change(next ? '门控已开启' : '门控已关闭');
      setGateOff(null);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const sync = async (item: QualitySource) => {
    setBusy(`sync-${item.id}`);
    try {
      await syncPlaylistSource(item.id);
      ok('已排入同步');
      broadcastQualityChanged();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const enable = async (item: QualitySource) => {
    setBusy(`enable-${item.id}`);
    try {
      await enableQualitySource(item.id);
      await change('源已启用');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const doDisable = async () => {
    if (!disable) return;
    setBusy(`disable-${disable.source.id}`);
    try {
      await disableQualitySource(disable.source.id, disable.keep);
      if (disable.keep && disable.remember) {
        localStorage.setItem('tv_buddy_quality_disable_keep', '1');
      } else if (!disable.keep) {
        localStorage.removeItem('tv_buddy_quality_disable_keep');
      }
      setDisable(null);
      await change(
        disable.keep ? '源已停用，频道保留在分组' : '源已停用，频道已移出分组',
      );
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const doRemove = async () => {
    if (!removing) return;
    setBusy(`remove-${removing.source.id}`);
    try {
      await deleteQualitySource(removing.source.id, removing.keep);
      setRemoving(null);
      await change(removing.keep ? '源已删除，频道转为本地频道' : '源已删除，频道已移出分组');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  if (!items) return <Empty text="正在读取来源…" icon="subs" />;

  const aggregates = configSources.filter((source) => source.mode !== 'sync');

  return (
    <>
      {node}
      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>同步源</h2>
            <span className="muted">只列托管源；关闭门控后由同步直接管理频道</span>
          </div>
          <div className="card-actions">
            <button className="ghost" onClick={() => void load()}>
              刷新
            </button>
          </div>
        </div>
        {items.length === 0 ? (
          <Empty text="没有同步来源" icon="subs" />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>来源</th>
                <th>状态</th>
                <th>门控</th>
                <th>最近同步 / 阶段</th>
                <th>准入</th>
                <th>最近移出</th>
                <th>最近一轮失败率</th>
                <th>操作</th>
              </tr>
            </thead>
            <tbody>
              {items.map((item) => (
                <tr key={item.id}>
                  <td>
                    {item.name || item.id}
                    <span className="cid">[{item.id}]</span>
                  </td>
                  <td>
                    <Badge tone={item.enabled ? 'ok' : 'muted'}>
                      {item.enabled ? '已启用' : '已停用'}
                    </Badge>
                  </td>
                  <td>
                    <Switch
                      checked={item.gate_enabled}
                      disabled={busy !== ''}
                      label={`${item.name || item.id} 门控`}
                      onChange={(next) =>
                        next ? void toggleGate(item, true) : setGateOff(item)
                      }
                    />
                  </td>
                  <td>
                    <div>{item.phase || '—'}</div>
                    <div className="hint">
                      {timeText(item.last_at)}
                      {item.truncated
                        ? `；本轮准入 ${item.admitted}，剩余 ${item.remaining} 个候选下一轮继续`
                        : ''}
                      {item.paused
                        ? `；${PAUSE_LABEL[item.pause_reason] ?? '本轮已暂停'}`
                        : ''}
                    </div>
                  </td>
                  <td>{item.channels}</td>
                  <td>{item.removed}</td>
                  <td>
                    {item.tested > 0
                      ? `${((item.removed / item.tested) * 100).toFixed(0)}%`
                      : '—'}
                  </td>
                  <td>
                    <button
                      className="chip"
                      disabled={busy !== '' || !item.enabled}
                      onClick={() => void sync(item)}
                    >
                      立即同步
                    </button>
                    {item.enabled ? (
                      <button
                        className="chip"
                        disabled={busy !== ''}
                        onClick={() => {
                          let remember = false;
                          try {
                            remember =
                              localStorage.getItem('tv_buddy_quality_disable_keep') === '1';
                          } catch {
                            // 隐私模式下读写失败时按未记住处理。
                          }
                          setDisable({
                            source: item,
                            keep: true,
                            remember,
                            confirm: false,
                          });
                        }}
                      >
                        停用
                      </button>
                    ) : (
                      <button
                        className="chip"
                        disabled={busy !== ''}
                        onClick={() => void enable(item)}
                      >
                        启用
                      </button>
                    )}
                    <button
                      className="chip"
                      title="到订阅源页编辑这个源"
                      onClick={() => {
                        window.location.hash = '#/subs';
                      }}
                    >
                      编辑
                    </button>
                    <button
                      className="chip danger-text"
                      disabled={busy !== ''}
                      onClick={() => setRemoving({ source: item, keep: true, confirm: false })}
                    >
                      删除
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </div>
      {aggregates.length > 0 ? (
        <div className="card">
          <details>
            <summary>聚合源不参与准入（{aggregates.length} 个）</summary>
            <p className="hint">
              聚合源只在生成总清单时拼接内容，没有成员同步与质量门控。
            </p>
            <ul>
              {aggregates.map((source) => (
                <li key={source.id}>{source.name || source.id}</li>
              ))}
            </ul>
          </details>
        </div>
      ) : null}
      {gateOff ? (
        <Confirm
          title="关闭门控？"
          text="关闭后，新频道不再校验，已准入频道不再周期移出。频道由同步直接管理。"
          confirmText="关闭门控"
          onCancel={() => setGateOff(null)}
          onConfirm={() => void toggleGate(gateOff, false)}
        />
      ) : null}
      {disable ? (
        <Modal
          title={`停用源「${disable.source.name || disable.source.id}」`}
          onClose={() => setDisable(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setDisable(null)}>
                取消
              </button>
              <button
                disabled={busy !== ''}
                onClick={() => {
                  if (disable.confirm || disable.keep) {
                    void doDisable();
                  } else {
                    setDisable({ ...disable, confirm: true });
                  }
                }}
              >
                {disable.confirm ? '确认移出' : '停用'}
              </button>
            </>
          }
        >
          {disable.confirm ? (
            <p>
              将移出 {disable.source.channels} 个频道。移出后不会自动恢复。
            </p>
          ) : (
            <>
              <p>停用后停止同步，并暂停该源的质量校验。</p>
              <div className="section-title">已准入频道</div>
              <label className="check">
                <input
                  type="radio"
                  checked={disable.keep}
                  onChange={() => setDisable({ ...disable, keep: true })}
                />
                <span>保留在分组（推荐）</span>
              </label>
              <label className="check">
                <input
                  type="radio"
                  checked={!disable.keep}
                  onChange={() => setDisable({ ...disable, keep: false })}
                />
                <span>同时移出分组</span>
              </label>
              <label className="check">
                <input
                  type="checkbox"
                  checked={disable.remember}
                  disabled={!disable.keep}
                  onChange={(event) =>
                    setDisable({ ...disable, remember: event.target.checked })
                  }
                />
                <span>记住我的选择（只记住保留）</span>
              </label>
            </>
          )}
        </Modal>
      ) : null}
      {removing ? (
        <Modal
          title={`删除源「${removing.source.name || removing.source.id}」`}
          onClose={() => setRemoving(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setRemoving(null)}>
                取消
              </button>
              <button
                disabled={busy !== ''}
                onClick={() => {
                  if (removing.confirm || removing.keep) {
                    void doRemove();
                  } else {
                    setRemoving({ ...removing, confirm: true });
                  }
                }}
              >
                {removing.confirm ? '确认删除并移出' : '删除'}
              </button>
            </>
          }
        >
          {removing.confirm ? (
            <p>
              将移出 {removing.source.channels} 个频道。删除后无法恢复源配置，频道不会自动恢复。
            </p>
          ) : (
            <>
              <p>删除后无法恢复源配置。</p>
              <div className="section-title">已准入频道</div>
              <label className="check">
                <input
                  type="radio"
                  checked={removing.keep}
                  onChange={() => setRemoving({ ...removing, keep: true })}
                />
                <span>保留为本地频道（推荐）</span>
              </label>
              <label className="check">
                <input
                  type="radio"
                  checked={!removing.keep}
                  onChange={() => setRemoving({ ...removing, keep: false })}
                />
                <span>随源删除</span>
              </label>
            </>
          )}
        </Modal>
      ) : null}
    </>
  );
}

/** 质量校验页。 */
export function QualityPage() {
  const { node, ok, err } = useToast();
  const [route, setRoute] = useState<QualityRoute>(() => qualityRoute());
  const [tab, setTab] = useState<Tab>(route.tab);
  const [status, setStatus] = useState<QualityStatus | null>(null);
  const [busy, setBusy] = useState('');
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [confirmRun, setConfirmRun] = useState(false);

  useEffect(() => {
    const onHash = () => {
      const next = qualityRoute();
      setRoute(next);
      setTab(next.tab);
    };
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  const changeTab = (next: Tab) => {
    setTab(next);
    setRoute({
      tab: next,
      gid: '',
      cid: '',
      source_id: '',
      action: '',
      reason: '',
      q: '',
    });
    const target = `#/quality/${next}`;
    if (window.location.hash !== target) {
      window.history.replaceState(null, '', target);
    }
  };

  const load = useCallback(async () => {
    try {
      setStatus(await fetchQualityStatus());
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), status?.running ? 2000 : 10000);
    return () => window.clearInterval(timer);
  }, [load, status?.running]);

  const run = async () => {
    setBusy('run');
    try {
      const result = await runQuality('all');
      ok(
        result.measure_only
          ? '质量已关闭，本次只测量'
          : `已排入质量校验（${result.run_id}）`,
      );
      await load();
      broadcastQualityChanged();
      changeTab('overview');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const cancel = async () => {
    setBusy('cancel');
    try {
      await cancelQuality();
      ok('取消中，已完成的结果保留');
      await load();
      broadcastQualityChanged();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
      setConfirmCancel(false);
    }
  };

  return (
    <>
      {node}
      <PageHead
        icon="activity"
        title="质量校验"
        sub="同步源按质量准入；手工频道按失败策略下线。"
        help="所有质量接口仅管理员可用。结果只记录哈希与指标，不保存完整地址。"
      >
        <button className="ghost" onClick={() => void load()} disabled={busy !== ''}>
          刷新
        </button>
        <button
          onClick={() => setConfirmRun(true)}
          disabled={busy !== '' || status?.running}
        >
          {busy === 'run' ? '已排入…' : '立即校验'}
        </button>
      </PageHead>
      <div className="tabs" role="tablist">
        {TABS.map(([id, label]) => (
          <button
            key={id}
            role="tab"
            aria-selected={tab === id}
            className={tab === id ? 'tab active' : 'tab'}
            onClick={() => changeTab(id)}
          >
            {label}
          </button>
        ))}
      </div>
      {tab === 'overview' && (
        <OverviewTab
          status={status}
          busy={busy !== ''}
          onRun={() => setConfirmRun(true)}
          onCancel={() => setConfirmCancel(true)}
        />
      )}
      {tab === 'rules' && <RulesTab status={status} onSaved={() => void load()} />}
      {tab === 'results' && (
        <ResultsTab
          running={status?.running === true}
          initialGid={route.gid}
          initialCid={route.cid}
          initialSourceId={route.source_id}
          initialAction={route.action}
          initialReason={route.reason}
          initialQuery={route.q}
        />
      )}
      {tab === 'sources' && <SourcesTab onChanged={() => void load()} />}
      {confirmRun ? (
        <Confirm
          title="立即校验全部频道？"
          text={
            status?.enabled
              ? '启用状态下，校验失败的频道会立即移出分组。已移出的频道需要重新同步源才能恢复。'
              : '质量校验已停用。本次只测量，不改变分组与订阅。'
          }
          confirmText="开始校验"
          onCancel={() => setConfirmRun(false)}
          onConfirm={() => {
            setConfirmRun(false);
            void run();
          }}
        />
      ) : null}
      {confirmCancel ? (
        <div className="modal-overlay" onClick={() => setConfirmCancel(false)}>
          <div className="modal" onClick={(event) => event.stopPropagation()}>
            <div className="modal-head">
              <h3>取消当前质量任务？</h3>
              <button
                className="icon-btn"
                onClick={() => setConfirmCancel(false)}
                aria-label="关闭"
              >
                <Icon name="close" size={15} />
              </button>
            </div>
            <div className="modal-body">
              <p>已提交的准入与移出不会回滚。未开始的频道会标记为取消。</p>
            </div>
            <div className="modal-foot">
              <button className="ghost" onClick={() => setConfirmCancel(false)}>
                继续运行
              </button>
              <button onClick={() => void cancel()} disabled={busy !== ''}>
                取消任务
              </button>
            </div>
          </div>
        </div>
      ) : null}
    </>
  );
}
