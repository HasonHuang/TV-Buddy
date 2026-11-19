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
  type QualityRun,
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

/** 运行状态徽章。先看是否进行中，再看结束原因。 */
function runBadge(status: QualityStatus): { tone: string; text: string } {
  const run = status.run;
  if (!run) return { tone: 'muted', text: '未运行' };
  if (status.running) return { tone: 'info', text: '进行中' };
  if (run.phase === 'failed' || run.error) return { tone: 'danger', text: '失败' };
  if (run.paused_by_guard) return { tone: 'warn', text: '安全阀暂停' };
  if (run.cancelled) return { tone: 'muted', text: '已取消' };
  if (run.measure_only) return { tone: 'info', text: '仅测量' };
  return { tone: 'ok', text: '已完成' };
}

/** 校验进度百分比。总数未知时返回 null，进度条显示为空。 */
function runPercent(run: QualityRun): number | null {
  if (run.total <= 0) return null;
  return Math.max(0, Math.min(100, Math.round((run.tested / run.total) * 100)));
}

function scheduleText(status: QualityStatus): string {
  if (status.schedule.mode === 'cron') return `cron ${status.schedule.cron}`;
  return `每 ${status.schedule.interval_hours} 小时`;
}

function sourcePhaseText(phase: string): string {
  return SOURCE_PHASE_LABEL[phase] ?? (phase || '—');
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

const TRIGGER_LABEL: Record<string, string> = {
  manual: '手动',
  schedule: '定时',
};

const RUN_PHASE_LABEL: Record<string, string> = {
  queued: '排队中',
  probing: '校验中',
  done: '已完成',
  failed: '失败',
  cancelled: '已取消',
  paused_by_guard: '安全阀暂停',
};

const SOURCE_PHASE_LABEL: Record<string, string> = {
  fetching: '抓取清单',
  parsing: '解析清单',
  gating: '质量门控',
  applying: '应用变更',
  done: '已完成',
  paused: '已暂停',
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

/** 状态条用的短时间。当天只显示时与分。 */
function shortTimeText(epoch: number): string {
  if (!Number.isFinite(epoch) || epoch <= 0) return '—';
  const date = new Date(epoch * 1000);
  const sameDay = date.toDateString() === new Date().toDateString();
  return date.toLocaleString('zh-CN', {
    ...(sameDay ? {} : { month: '2-digit', day: '2-digit' }),
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  });
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

function emptyRoute(tab: Tab): QualityRoute {
  return {
    tab,
    gid: '',
    cid: '',
    source_id: '',
    action: '',
    reason: '',
    q: '',
  };
}

/** 生成地址栏哈希。只写入非空筛选，地址保持短。 */
function routeHash(route: QualityRoute): string {
  const params = new URLSearchParams();
  if (route.source_id) params.set('source_id', route.source_id);
  if (route.gid) params.set('gid', route.gid);
  if (route.cid) params.set('cid', route.cid);
  if (route.action) params.set('action', route.action);
  if (route.reason) params.set('reason', route.reason);
  if (route.q.trim()) params.set('q', route.q.trim());
  const query = params.toString();
  return `#/quality/${route.tab}${query ? `?${query}` : ''}`;
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

/** 顶部状态条：所有标签共用，显示开关、调度、进度与告警。 */
function QualityStatusBar({
  status,
  busy,
  onOpenOverview,
  onCancel,
}: {
  status: QualityStatus | null;
  busy: boolean;
  onOpenOverview: () => void;
  onCancel: () => void;
}) {
  if (!status) return null;
  const run = status.run;
  const percent = run ? runPercent(run) : null;
  const warnings =
    status.source_errors.length +
    (status.guard.tripped ? 1 : 0) +
    (status.counts.probe_errors_7d > 0 ? 1 : 0);
  return (
    <section className="quality-status" aria-label="质量校验状态">
      <div className="quality-status-main">
        <Badge tone={status.enabled ? 'ok' : 'muted'}>
          {status.enabled ? '已开启' : '已关闭'}
        </Badge>
        <span className="quality-status-text">{scheduleText(status)}</span>
        <span className="quality-status-dot" aria-hidden="true">
          ·
        </span>
        <span className="quality-status-text">
          下次运行 {shortTimeText(status.schedule.next_run_at)}
        </span>
        {!status.enabled ? (
          <span className="quality-status-text">手动运行只测量</span>
        ) : null}
      </div>
      <div className="quality-status-run">
        {status.running && run ? (
          <>
            <div className="quality-run-progress-head">
              <span>正在校验</span>
              <span className="mono">
                {run.tested} / {run.total}
              </span>
            </div>
            <div
              className={`quality-run-bar${percent === null ? ' empty' : ''}`}
              role="progressbar"
              aria-label="校验进度"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={percent ?? undefined}
              aria-valuetext={`已测 ${run.tested} / ${run.total}`}
            >
              <span style={{ width: `${percent ?? 0}%` }} />
            </div>
          </>
        ) : run ? (
          <span className="quality-status-text">
            上次运行 {relativeText(run.started_at)} · 准入 {run.admitted} / 移出{' '}
            {run.removed}
          </span>
        ) : (
          <span className="quality-status-text">还没有运行记录</span>
        )}
      </div>
      {warnings > 0 ? (
        <button
          type="button"
          className="quality-status-alert"
          title="查看概览中的警告"
          onClick={onOpenOverview}
        >
          <Badge tone={status.guard.tripped ? 'danger' : 'warn'}>
            {warnings} 项需要处理
          </Badge>
        </button>
      ) : null}
      {status.running ? (
        <button className="ghost" onClick={onCancel} disabled={busy}>
          取消任务
        </button>
      ) : null}
    </section>
  );
}

/** 校验进度：已测数量与百分比。 */
function RunProgress({ run }: { run: QualityRun }) {
  const percent = runPercent(run);
  return (
    <div className="quality-run-progress">
      <div className="quality-run-progress-head">
        <span>
          已测 {run.tested} / {run.total}
        </span>
        <strong>{percent === null ? '准备中' : `${percent}%`}</strong>
      </div>
      <div
        className={`quality-run-bar${percent === null ? ' empty' : ''}`}
        role="progressbar"
        aria-label="校验进度"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={percent ?? undefined}
        aria-valuetext={`已测 ${run.tested} / ${run.total}`}
      >
        <span style={{ width: `${percent ?? 0}%` }} />
      </div>
    </div>
  );
}

/** 概览标签。 */
function OverviewTab({
  status,
  onRun,
  onOpenRules,
  onOpenSources,
  onOpenResults,
}: {
  status: QualityStatus | null;
  onRun: () => void;
  onOpenRules: () => void;
  onOpenSources: () => void;
  onOpenResults: (params?: Partial<QualityRoute>) => void;
}) {
  if (!status) return <Empty text="正在读取质量状态…" icon="activity" />;
  const run = status.run;
  const badge = runBadge(status);
  const warnings: Array<{
    key: string;
    tone: string;
    badge: string;
    text: string;
    action: string;
    onAction: () => void;
  }> = [];
  if (status.guard.tripped) {
    warnings.push({
      key: 'guard',
      tone: 'danger',
      badge: '安全阀',
      text: `上一轮移出达到阈值，成员变更已暂停。阈值：${(
        status.guard.max_remove_ratio * 100
      ).toFixed(0)}%，至少 ${status.guard.min_remove_count} 个。`,
      action: '查看规则',
      onAction: onOpenRules,
    });
  }
  for (const source of status.source_errors) {
    warnings.push({
      key: `source-${source.id}`,
      tone: 'warn',
      badge: '源抓取',
      text: `${source.name || source.id}：${source.error}`,
      action: '查看来源',
      onAction: onOpenSources,
    });
  }
  if (status.counts.probe_errors_7d > 0) {
    warnings.push({
      key: 'probe',
      tone: 'warn',
      badge: '探测错误',
      text: `近 7 天有 ${status.counts.probe_errors_7d} 条探测错误，请检查解析脚本或探测地址。`,
      action: '查看结果',
      onAction: () => onOpenResults({ reason: 'probe_error' }),
    });
  }
  return (
    <>
      {warnings.length > 0 ? (
        <Card title="需要处理" note={`${warnings.length} 项，按影响排序`}>
          <ul className="quality-warnings">
            {warnings.map((item) => (
              <li key={item.key}>
                <Badge tone={item.tone}>{item.badge}</Badge>
                <span title={item.text}>{item.text}</span>
                <button type="button" className="quality-link" onClick={item.onAction}>
                  {item.action}
                </button>
              </li>
            ))}
          </ul>
        </Card>
      ) : null}
      <div className="stats">
        <div className="stat">
          <div className="k">当前准入</div>
          <div className="v">{status.counts.admitted}</div>
          <div className="hint">结果表中状态为准入的频道数</div>
        </div>
        <div className="stat">
          <div className="k">近 7 天移出</div>
          <div className="v">{status.counts.removed_7d}</div>
          <div className="hint">按结果表的移出记录统计</div>
        </div>
        <div className="stat">
          <div className="k">近 7 天探测错误</div>
          <div className={status.counts.probe_errors_7d > 0 ? 'v danger' : 'v'}>
            {status.counts.probe_errors_7d}
          </div>
          <div className="hint">检查解析脚本或探测地址</div>
        </div>
        <div className="stat">
          <div className="k">同步源</div>
          <div className="v">{status.counts.sources}</div>
          <div className="hint">已配置的同步源</div>
        </div>
      </div>
      <div className="quality-overview-grid">
        <div className="quality-overview-main">
          <Card
            title="最近任务"
            note={
              run
                ? `${TRIGGER_LABEL[run.trigger] ?? run.trigger} · ${
                    RUN_PHASE_LABEL[run.phase] ?? (run.phase || '—')
                  }`
                : '还没有运行记录'
            }
            actions={<Badge tone={badge.tone}>{badge.text}</Badge>}
          >
            {run ? (
              <>
                <RunProgress run={run} />
                <div className="quality-run-metrics">
                  <div>
                    <span>准入</span>
                    <strong>{run.admitted}</strong>
                  </div>
                  <div>
                    <span>移出</span>
                    <strong>{run.removed}</strong>
                  </div>
                  <div>
                    <span>拒绝</span>
                    <strong>{run.rejected}</strong>
                  </div>
                  <div>
                    <span>跳过</span>
                    <strong>{run.skipped}</strong>
                  </div>
                </div>
                <dl className="quality-run-meta">
                  <dt>开始</dt>
                  <dd>{timeText(run.started_at)}</dd>
                  <dt>流量</dt>
                  <dd>{bytesText(run.bytes)}</dd>
                  <dt>任务</dt>
                  <dd className="mono">{run.run_id}</dd>
                </dl>
                {run.measure_only ? (
                  <div className="msg info">
                    质量已关闭，本次只测量，不改变分组与订阅。
                  </div>
                ) : null}
                {run.paused_by_guard ? (
                  <div className="msg warn">
                    本轮移出达到安全阀阈值，成员变更已暂停。
                  </div>
                ) : null}
                {run.cancelled ? (
                  <div className="msg info">任务已取消，已完成的结果保留。</div>
                ) : null}
                {run.error ? <div className="msg err">{run.error}</div> : null}
              </>
            ) : (
              <Empty
                text={
                  status.running ? '正在校验，结果稍后显示' : '还没有运行记录'
                }
                icon="activity"
                action={
                  status.running ? undefined : (
                    <button type="button" onClick={onRun}>
                      立即校验
                    </button>
                  )
                }
              />
            )}
          </Card>
        </div>
        <aside className="quality-overview-rail">
          <Card
            title="安全阀"
            note="移出过多时暂停成员变更"
            actions={
              <Badge
                tone={
                  status.guard.tripped
                    ? 'danger'
                    : status.guard.enabled
                      ? 'ok'
                      : 'muted'
                }
              >
                {status.guard.tripped
                  ? '已触发'
                  : status.guard.enabled
                    ? '已开启'
                    : '已关闭'}
              </Badge>
            }
          >
            <dl>
              <dt>触发条件</dt>
              <dd>
                移出比例超过 {(status.guard.max_remove_ratio * 100).toFixed(0)}%，且至少{' '}
                {status.guard.min_remove_count} 个
              </dd>
            </dl>
            <div className="quality-card-foot">
              <button type="button" className="ghost" onClick={onOpenRules}>
                调整规则
              </button>
            </div>
          </Card>
          <Card title="分辨率探测" note="完整档读取切片开头识别分辨率">
            <dl>
              <dt>ffprobe</dt>
              <dd>
                <Badge tone={status.ffprobe.installed ? 'ok' : 'muted'}>
                  {status.ffprobe.installed ? '已安装' : '未安装'}
                </Badge>
              </dd>
              <dt>识别</dt>
              <dd>
                <Badge tone={status.ffprobe.enabled ? 'ok' : 'muted'}>
                  {status.ffprobe.enabled ? '已启用' : '已关闭'}
                </Badge>
              </dd>
            </dl>
            {status.ffprobe.note ? (
              <div className="hint">{status.ffprobe.note}</div>
            ) : null}
            <div className="quality-card-foot">
              <button type="button" className="ghost" onClick={onOpenRules}>
                查看规则
              </button>
            </div>
          </Card>
        </aside>
      </div>
    </>
  );
}

/** 规则标签。 */
function RulesTab({
  status,
  onSaved,
  onDirtyChange,
}: {
  status: QualityStatus | null;
  onSaved: () => void;
  onDirtyChange: (dirty: boolean) => void;
}) {
  const { node, ok, err } = useToast();
  const [draft, setDraft] = useState<Cfg | null>(null);
  const [original, setOriginal] = useState<Cfg | null>(null);
  const [busy, setBusy] = useState('');
  const [preview, setPreview] = useState<number[]>([]);
  const [previewError, setPreviewError] = useState('');
  const [speedRows, setSpeedRows] = useState<SpeedRow[]>([]);
  const [confirmDiscard, setConfirmDiscard] = useState(false);

  const load = useCallback(async () => {
    try {
      const config = await api<Cfg>('GET', '/config');
      setDraft(config);
      setOriginal(config);
      setSpeedRows(readSpeedRows(config));
      setConfirmDiscard(false);
      onDirtyChange(false);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err, onDirtyChange]);

  useEffect(() => {
    void load();
  }, [load]);

  // 离开规则页时清掉脏标记。切页确认由页面容器处理。
  useEffect(() => () => onDirtyChange(false), [onDirtyChange]);

  const update = (path: string, value: unknown) => {
    setDraft((prev) => (prev ? setPath(prev, path, value) : prev));
  };

  const commitSpeedRows = (next: SpeedRow[]) => {
    setSpeedRows(next);
    update('quality.rules.resolution_speed_map', speedRowsToMap(next));
  };

  /** 只有质量子树参与比较。分组与频道很多时，整份配置比较太贵。 */
  const dirty = useMemo(
    () =>
      draft !== null &&
      original !== null &&
      JSON.stringify(getPath(draft, 'quality')) !==
        JSON.stringify(getPath(original, 'quality')),
    [draft, original],
  );

  useEffect(() => {
    onDirtyChange(dirty);
  }, [dirty, onDirtyChange]);

  const discard = async () => {
    setConfirmDiscard(false);
    setBusy('discard');
    await load();
    setBusy('');
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
      const saved = { ...draft, revision };
      setDraft(saved);
      setOriginal(saved);
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
  const speedRule =
    asNumber(draft, 'quality.rules.min_speed_mib') > 0 ||
    asNumber(draft, 'quality.rules.fallback_speed_mib') > 0 ||
    speedRows.some((row) => Number(row.speed) > 0);
  const resolutionRule =
    asText(draft, 'quality.rules.min_resolution').trim() !== '' ||
    asText(draft, 'quality.rules.max_resolution').trim() !== '';
  // 对齐后端 `required_probe_mode`：完整档不因规则降级，只在 ffprobe 关闭时降为测速。
  const actualProbe =
    configuredProbe === 'full'
      ? ffprobeEnabled
        ? 'full'
        : 'speed'
      : speedRule
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
            <button
              className="ghost"
              onClick={() => (dirty ? setConfirmDiscard(true) : void load())}
              disabled={busy !== ''}
            >
              {dirty ? '放弃更改' : '重新读取'}
            </button>
            <button
              onClick={() => void save()}
              disabled={busy !== '' || !dirty}
              title={dirty ? undefined : '没有未保存的更改'}
            >
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
              onChange={(next) => {
                update('quality.schedule', next);
                setPreview([]);
                setPreviewError('');
              }}
            />
          </Field>
          {scheduleMode === 'interval' ? (
            <Field label="间隔小时" hint="1–8760">
              <NumberInput
                min={1}
                max={8760}
                value={asNumber(draft, 'quality.interval_hours')}
                onChange={(value) => update('quality.interval_hours', value)}
              />
            </Field>
          ) : (
            <Field label="cron 表达式" hint="分 时 日 月 周，按服务器时区">
              <input
                value={asText(draft, 'quality.cron')}
                onChange={(event) => update('quality.cron', event.target.value)}
                placeholder="0 */6 * * *"
              />
            </Field>
          )}
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
        {previewError ? <div className="msg err">{previewError}</div> : null}
        {preview.length > 0 ? (
          <div className="quality-preview">
            <span className="muted">未来运行</span>
            <div className="quality-preview-times">
              {preview.map((time) => (
                <span className="badge info" key={time}>
                  {timeText(time)}
                </span>
              ))}
            </div>
          </div>
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
      {dirty ? (
        <div
          className="quality-save-bar"
          role="region"
          aria-label="未保存的规则更改"
        >
          <div className="quality-save-text">
            <Badge tone="warn">未保存</Badge>
            <span>规则更改尚未生效。</span>
          </div>
          <div className="card-actions">
            <button
              className="ghost"
              onClick={() => setConfirmDiscard(true)}
              disabled={busy !== ''}
            >
              放弃更改
            </button>
            <button onClick={() => void save()} disabled={busy !== ''}>
              {busy === 'save' ? '保存中…' : '保存规则'}
            </button>
          </div>
        </div>
      ) : null}
      {confirmDiscard ? (
        <Confirm
          title="放弃未保存的更改？"
          text="将重新读取已保存的规则。未保存的更改会丢失。"
          confirmText="放弃更改"
          onCancel={() => setConfirmDiscard(false)}
          onConfirm={() => void discard()}
        />
      ) : null}
    </>
  );
}

/** 结果标签。 */
function ResultsTab({
  running,
  enabled,
  onRun,
  initialGid,
  initialCid,
  initialSourceId,
  initialAction,
  initialReason,
  initialQuery,
}: {
  running: boolean;
  enabled: boolean;
  onRun: () => void;
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
  const [debouncedQ, setDebouncedQ] = useState(initialQuery);
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
    setDebouncedQ(initialQuery);
    setOffset(0);
  }, [
    initialGid,
    initialCid,
    initialSourceId,
    initialAction,
    initialReason,
    initialQuery,
  ]);

  // 搜索输入防抖。每敲一个字都请求接口会浪费流量。
  useEffect(() => {
    const timer = window.setTimeout(() => setDebouncedQ(filters.q), 250);
    return () => window.clearTimeout(timer);
  }, [filters.q]);

  // 请求只用防抖后的搜索词；其它筛选立即生效。
  const appliedFilters = useMemo(
    () => ({ ...filters, q: debouncedQ }),
    [
      filters.source_id,
      filters.gid,
      filters.cid,
      filters.action,
      filters.reason,
      debouncedQ,
    ],
  );

  // 筛选写入地址栏，刷新、复制链接与返回时都能恢复。
  useEffect(() => {
    window.history.replaceState(null, '', routeHash({ tab: 'results', ...filters }));
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
        ...appliedFilters,
        limit,
        offset,
      });
      if (seq === requestSeq.current) setData(body);
    } catch (error) {
      if (seq === requestSeq.current) {
        err(error instanceof Error ? error.message : String(error));
      }
    }
  }, [appliedFilters, err, limit, offset]);

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

  const clearFilters = () => {
    setFilters({ source_id: '', gid: '', cid: '', action: '', reason: '', q: '' });
    setDebouncedQ('');
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
      <Card
        title="结果"
        note={`共 ${total} 条${running ? ' · 任务进行中，自动刷新' : ''}`}
        actions={
          <button className="ghost" onClick={() => void load()}>
            <Icon name="refresh" size={14} />
            刷新
          </button>
        }
      >
        <div className="filter-bar quality-filters">
          <input
            className="search"
            value={filters.q}
            aria-label="搜索频道名或 ID"
            placeholder="搜索频道名或 ID"
            onChange={(event) => setFilter('q', event.target.value)}
          />
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
          {filters.cid ? (
            <span className="badge info quality-scope">
              频道 {filters.cid}
              <button
                type="button"
                className="icon-btn"
                aria-label="清除频道筛选"
                onClick={() => {
                  setFilter('gid', '');
                  setFilter('cid', '');
                }}
              >
                <Icon name="close" size={12} />
              </button>
            </span>
          ) : null}
          {filtered ? (
            <button className="ghost" onClick={clearFilters}>
              清除筛选
            </button>
          ) : null}
        </div>
        {items.length === 0 ? (
          <Empty
            text={
              filtered
                ? '没有匹配结果'
                : running
                  ? '正在校验，结果稍后显示'
                  : '还没有结果记录'
            }
            icon="activity"
            action={
              filtered ? (
                <button type="button" className="ghost" onClick={clearFilters}>
                  清除筛选
                </button>
              ) : running ? undefined : (
                <button type="button" onClick={onRun}>
                  立即校验
                </button>
              )
            }
          />
        ) : (
          <>
            <div className="table-wrap">
              <table
                className="table quality-results-table"
                aria-label="质量校验结果"
              >
                <thead>
                  <tr>
                    <th>频道</th>
                    <th>动作</th>
                    <th>原因</th>
                    <th>来源</th>
                    <th>目标分组</th>
                    <th className="right">延迟</th>
                    <th className="right">速率</th>
                    <th className="right">分辨率</th>
                    <th>最近校验</th>
                    <th>操作</th>
                  </tr>
                </thead>
                <tbody>
                  {items.map((item) => (
                    <tr key={item.key}>
                      <td>
                        <span className="quality-channel-name">
                          {item.name || item.cid}
                        </span>
                        <span className="cid">[{item.cid}]</span>
                      </td>
                      <td>
                        <Badge tone={resultActionTone(item)}>
                          {resultActionText(item)}
                        </Badge>
                      </td>
                      <td>{REASON_LABEL[item.reason] ?? item.reason}</td>
                      <td>{item.source_id ? sourceName(item.source_id) : '手工'}</td>
                      <td>{item.target_group || '—'}</td>
                      <td className="right num">
                        {item.delay_ms === null ? '—' : `${item.delay_ms} ms`}
                      </td>
                      <td className="right num">
                        {item.speed_mib === null
                          ? '—'
                          : `${item.speed_mib.toFixed(1)} MiB/s`}
                      </td>
                      <td className="right num">{resolutionText(item.resolution)}</td>
                      <td title={timeText(item.checked_at)}>
                        {relativeText(item.checked_at)}
                      </td>
                      <td>
                        <div className="quality-row-actions">
                          <button className="chip" onClick={() => setDetail(item)}>
                            详情
                          </button>
                          {item.action === 'admitted' ? (
                            <button className="chip" onClick={() => setRerun(item)}>
                              重新校验
                            </button>
                          ) : null}
                        </div>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
            <div className="quality-table-foot">
              <span className="muted">
                第 {total === 0 ? 0 : offset + 1}–{pageEnd} 条，共 {total} 条
              </span>
              <div className="quality-pager" role="group" aria-label="分页">
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
      </Card>
      {detail ? (
        <Modal
          title="质量结果详情"
          onClose={() => setDetail(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setDetail(null)}>
                关闭
              </button>
              {detail.action === 'admitted' ? (
                <button
                  className="ghost"
                  onClick={() => {
                    setRerun(detail);
                    setDetail(null);
                  }}
                >
                  重新校验
                </button>
              ) : null}
            </>
          }
        >
          <div className="quality-detail-head">
            <div className="quality-detail-name">
              {detail.name || detail.cid}
              <span className="cid">[{detail.cid}]</span>
            </div>
            <div className="quality-detail-badges">
              <Badge tone={resultActionTone(detail)}>
                {resultActionText(detail)}
              </Badge>
              <Badge tone="muted">
                {REASON_LABEL[detail.reason] ?? detail.reason}
              </Badge>
            </div>
          </div>
          <dl>
            <dt>来源</dt>
            <dd>{detail.source_id ? sourceName(detail.source_id) : '手工'}</dd>
            <dt>目标分组</dt>
            <dd>{detail.target_group || '—'}</dd>
            <dt>最近校验</dt>
            <dd>{timeText(detail.checked_at)}</dd>
            <dt>首次失败</dt>
            <dd>{timeText(detail.first_fail_at ?? 0)}</dd>
            <dt>延迟</dt>
            <dd>{detail.delay_ms === null ? '—' : `${detail.delay_ms} ms`}</dd>
            <dt>速率</dt>
            <dd>
              {detail.speed_mib === null
                ? '—'
                : `${detail.speed_mib.toFixed(2)} MiB/s`}
            </dd>
            <dt>分辨率</dt>
            <dd>{detail.resolution || '—'}</dd>
            <dt>HTTP 状态</dt>
            <dd>{detail.http_status ?? '—'}</dd>
            <dt>地址哈希</dt>
            <dd className="mono">{detail.url_hash || '—'}</dd>
            <dt>规则哈希</dt>
            <dd className="mono">{detail.rule_hash || '—'}</dd>
          </dl>
          {detail.action === 'removed' || detail.action === 'source_removed' ? (
            <p className="hint">需要重新同步源才能再次准入。</p>
          ) : null}
        </Modal>
      ) : null}
      {rerun ? (
        <Confirm
          title="重新校验这个频道？"
          text={`将校验「${rerun.name || rerun.cid}」。${
            enabled ? '失败会立即从分组移出。' : '质量已关闭，本次只测量。'
          }`}
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
              <Icon name="refresh" size={14} />
              刷新
            </button>
          </div>
        </div>
        {items.length === 0 ? (
          <Empty
            text="没有同步来源"
            icon="subs"
            action={
              <button
                type="button"
                className="ghost"
                onClick={() => {
                  window.location.hash = '#/subs';
                }}
              >
                前往订阅源
              </button>
            }
          />
        ) : (
          <div className="table-wrap">
            <table
              className="table quality-sources-table"
              aria-label="同步源质量门控状态"
            >
              <thead>
                <tr>
                  <th>来源</th>
                  <th>状态</th>
                  <th>门控</th>
                  <th>最近阶段</th>
                  <th className="right">准入</th>
                  <th className="right">移出</th>
                  <th className="right">移出率</th>
                  <th>操作</th>
                </tr>
              </thead>
              <tbody>
                {items.map((item) => (
                  <tr key={item.id}>
                    <td>
                      <div className="quality-source-name">{item.name || item.id}</div>
                      <span className="cid">[{item.id}]</span>
                      {item.error ? (
                        <div className="hint quality-source-error" title={item.error}>
                          {item.error}
                        </div>
                      ) : null}
                    </td>
                    <td>
                      <div className="quality-source-badges">
                        <Badge tone={item.enabled ? 'ok' : 'muted'}>
                          {item.enabled ? '已启用' : '已停用'}
                        </Badge>
                        {item.paused ? <Badge tone="warn">本轮暂停</Badge> : null}
                        {item.truncated ? <Badge tone="info">下轮继续</Badge> : null}
                        {item.error ? <Badge tone="danger">抓取错误</Badge> : null}
                      </div>
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
                      <div>{sourcePhaseText(item.phase)}</div>
                      <div className="hint">
                        {item.last_at ? timeText(item.last_at) : '尚未同步'}
                        {item.truncated
                          ? `；本轮准入 ${item.admitted}，剩余 ${item.remaining} 个下一轮继续`
                          : ''}
                        {item.paused
                          ? `；${PAUSE_LABEL[item.pause_reason] ?? '本轮已暂停'}`
                          : ''}
                      </div>
                    </td>
                    <td className="right num">{item.channels}</td>
                    <td className="right num">{item.removed}</td>
                    <td
                      className="right num"
                      title={
                        item.tested > 0
                          ? `最近一轮已测 ${item.tested} 个`
                          : undefined
                      }
                    >
                      {item.tested > 0
                        ? `${((item.removed / item.tested) * 100).toFixed(0)}%`
                        : '—'}
                    </td>
                    <td>
                      <div className="quality-row-actions">
                        <button
                          className="chip"
                          disabled={busy !== '' || !item.enabled}
                          title="按当前规则同步这个源"
                          onClick={() => void sync(item)}
                        >
                          立即同步
                        </button>
                        {item.enabled ? (
                          <button
                            className="chip"
                            disabled={busy !== ''}
                            title="停用这个源"
                            onClick={() => {
                              let remember = false;
                              try {
                                remember =
                                  localStorage.getItem(
                                    'tv_buddy_quality_disable_keep',
                                  ) === '1';
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
                            title="启用这个源"
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
                          title="删除这个源"
                          onClick={() =>
                            setRemoving({ source: item, keep: true, confirm: false })
                          }
                        >
                          删除
                        </button>
                      </div>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
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
export function QualityPage({
  onDirtyChange,
}: {
  onDirtyChange?: (dirty: boolean) => void;
}) {
  const { node, ok, err } = useToast();
  const [route, setRoute] = useState<QualityRoute>(() => qualityRoute());
  const [tab, setTab] = useState<Tab>(route.tab);
  const [status, setStatus] = useState<QualityStatus | null>(null);
  const [busy, setBusy] = useState('');
  const [confirmCancel, setConfirmCancel] = useState(false);
  const [confirmRun, setConfirmRun] = useState(false);
  const [rulesDirty, setRulesDirty] = useState(false);
  const [pendingRoute, setPendingRoute] = useState<QualityRoute | null>(null);
  const rulesDirtyRef = useRef(false);
  const tabRef = useRef(tab);
  const routeRef = useRef(route);

  const markRulesDirty = useCallback((dirty: boolean) => {
    rulesDirtyRef.current = dirty;
    setRulesDirty(dirty);
  }, []);

  useEffect(() => {
    tabRef.current = tab;
  }, [tab]);

  useEffect(() => {
    routeRef.current = route;
  }, [route]);

  // 把未保存状态交给外层导航，切分区前先确认。
  useEffect(() => {
    onDirtyChange?.(rulesDirty);
  }, [onDirtyChange, rulesDirty]);

  const applyRoute = useCallback((next: QualityRoute) => {
    tabRef.current = next.tab;
    routeRef.current = next;
    setRoute(next);
    setTab(next.tab);
    const target = routeHash(next);
    if (window.location.hash !== target) {
      window.history.replaceState(null, '', target);
    }
  }, []);

  /** 切标签。规则页有未保存修改时先确认。 */
  const requestTab = useCallback(
    (next: Tab) => {
      if (next === tabRef.current) return;
      const nextRoute = emptyRoute(next);
      if (tabRef.current === 'rules' && rulesDirtyRef.current) {
        setPendingRoute(nextRoute);
        return;
      }
      applyRoute(nextRoute);
    },
    [applyRoute],
  );

  /** 打开结果页，可带筛选。规则页有未保存修改时先确认。 */
  const openResults = useCallback(
    (params: Partial<QualityRoute> = {}) => {
      const nextRoute: QualityRoute = {
        ...emptyRoute('results'),
        ...params,
        tab: 'results',
      };
      if (tabRef.current === 'rules' && rulesDirtyRef.current) {
        setPendingRoute(nextRoute);
        return;
      }
      applyRoute(nextRoute);
    },
    [applyRoute],
  );

  // 浏览器前进/后退：规则页有未保存修改时先确认。
  useEffect(() => {
    const onHash = (event: HashChangeEvent) => {
      const nextHash = new URL(event.newURL).hash;
      // 切到别的分区时由面板的导航守卫处理，这里只管质量页内部。
      if (!nextHash.startsWith('#/quality')) return;
      const next = qualityRoute(nextHash);
      if (
        tabRef.current === 'rules' &&
        next.tab !== 'rules' &&
        rulesDirtyRef.current
      ) {
        window.history.replaceState(
          null,
          '',
          routeHash({ ...routeRef.current, tab: 'rules' }),
        );
        setPendingRoute(next);
        return;
      }
      setRoute(next);
      setTab(next.tab);
    };
    window.addEventListener('hashchange', onHash);
    return () => window.removeEventListener('hashchange', onHash);
  }, []);

  // 关闭或刷新页面时提示未保存的规则。
  useEffect(() => {
    if (!rulesDirty) return;
    const onBeforeUnload = (event: BeforeUnloadEvent) => {
      event.preventDefault();
      event.returnValue = '';
    };
    window.addEventListener('beforeunload', onBeforeUnload);
    return () => window.removeEventListener('beforeunload', onBeforeUnload);
  }, [rulesDirty]);

  const discardAndGo = () => {
    const next = pendingRoute;
    setPendingRoute(null);
    markRulesDirty(false);
    if (next) applyRoute(next);
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

  const onTabKeyDown = (
    event: React.KeyboardEvent<HTMLButtonElement>,
    index: number,
  ) => {
    let nextIndex = -1;
    if (event.key === 'ArrowRight') nextIndex = (index + 1) % TABS.length;
    else if (event.key === 'ArrowLeft') {
      nextIndex = (index - 1 + TABS.length) % TABS.length;
    } else if (event.key === 'Home') nextIndex = 0;
    else if (event.key === 'End') nextIndex = TABS.length - 1;
    if (nextIndex < 0) return;
    event.preventDefault();
    const next = TABS[nextIndex][0];
    const blocked =
      tabRef.current === 'rules' && rulesDirtyRef.current && next !== 'rules';
    requestTab(next);
    if (!blocked) {
      window.requestAnimationFrame(() => {
        document.getElementById(`quality-tab-${next}`)?.focus();
      });
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
        <button
          className="ghost"
          onClick={() => void load()}
          disabled={busy !== ''}
          title="重新读取质量状态"
        >
          <Icon name="refresh" size={14} />
          刷新状态
        </button>
        <button
          onClick={() => setConfirmRun(true)}
          disabled={busy !== '' || status?.running}
        >
          {status?.running
            ? '校验进行中'
            : busy === 'run'
              ? '已排入…'
              : '立即校验'}
        </button>
      </PageHead>
      <QualityStatusBar
        status={status}
        busy={busy !== ''}
        onOpenOverview={() => requestTab('overview')}
        onCancel={() => setConfirmCancel(true)}
      />
      <div className="tabs" role="tablist" aria-label="质量校验视图">
        {TABS.map(([id, label], index) => (
          <button
            key={id}
            id={`quality-tab-${id}`}
            role="tab"
            aria-selected={tab === id}
            aria-controls="quality-panel"
            tabIndex={tab === id ? 0 : -1}
            className={tab === id ? 'tab active' : 'tab'}
            onClick={() => requestTab(id)}
            onKeyDown={(event) => onTabKeyDown(event, index)}
          >
            {label}
            {id === 'rules' && rulesDirty ? (
              <>
                <span className="tab-dot" aria-hidden="true" />
                <span className="sr-only">有未保存的更改</span>
              </>
            ) : null}
          </button>
        ))}
      </div>
      <div
        className="quality-panel"
        role="tabpanel"
        id="quality-panel"
        aria-labelledby={`quality-tab-${tab}`}
      >
        {tab === 'overview' && (
          <OverviewTab
            status={status}
            onRun={() => setConfirmRun(true)}
            onOpenRules={() => requestTab('rules')}
            onOpenSources={() => requestTab('sources')}
            onOpenResults={openResults}
          />
        )}
        {tab === 'rules' && (
          <RulesTab
            status={status}
            onSaved={() => void load()}
            onDirtyChange={markRulesDirty}
          />
        )}
        {tab === 'results' && (
          <ResultsTab
            running={status?.running === true}
            enabled={status?.enabled === true}
            onRun={() => setConfirmRun(true)}
            initialGid={route.gid}
            initialCid={route.cid}
            initialSourceId={route.source_id}
            initialAction={route.action}
            initialReason={route.reason}
            initialQuery={route.q}
          />
        )}
        {tab === 'sources' && <SourcesTab onChanged={() => void load()} />}
      </div>
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
        <Confirm
          title="取消当前质量任务？"
          text="已提交的准入与移出不会回滚。未开始的频道会标记为取消。"
          confirmText="取消任务"
          busy={busy === 'cancel'}
          onCancel={() => setConfirmCancel(false)}
          onConfirm={() => void cancel()}
        />
      ) : null}
      {pendingRoute ? (
        <Confirm
          title="离开规则页？"
          text="当前有未保存的规则更改。离开后这些更改会丢失。"
          confirmText="放弃更改并离开"
          onCancel={() => setPendingRoute(null)}
          onConfirm={discardAndGo}
        />
      ) : null}
    </>
  );
}
