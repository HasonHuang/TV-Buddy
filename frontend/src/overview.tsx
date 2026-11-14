/** 概览页：播放流量、资源占用与系统摘要。
 *
 * 数据来源：`/status`（每 5 秒轮询，供 KPI 与资源卡）、
 * `/metrics/history`（趋势图，SQLite 历史）、`/stats` 与 `/schedule`（可选）。
 */

import { useCallback, useEffect, useId, useMemo, useRef, useState } from 'react';

import { api } from './api';
import { Icon, type IconName } from './icons';
import { MODES } from './modes';
import { STATUS_LABELS, STATUS_TONE } from './schedule';
import { SystemPage } from './system';
import { PageHead, useToast } from './ui';

type Status = Record<string, unknown>;

/** `GET /panel/api/stats` 的字段子集。 */
type PlayStats = {
  online_window_secs?: number;
  online_viewers?: number;
  active_channels?: number;
  channels?: {
    gid: string;
    cid: string;
    name: string;
    online: number;
    requests: number;
  }[];
};

/** `GET /panel/api/schedule` 的字段子集。 */
type Task = {
  id: string;
  name: string;
  status: string;
  start_time: string;
  end_time: string;
  error: string;
};

/** `GET /panel/api/dash/status` 的单条会话。 */
type DashSession = {
  stream_key: string;
  phase: string;
  segment_count: number;
  uptime_secs: number;
};

/** 一次 `/status` 采样。KPI 用它的差值。 */
type Sample = {
  at: number;
  /** 累计出网字节。 */
  out: number;
  /** 进程累计 CPU 秒。 */
  cpu: number;
};

/** `GET /panel/api/metrics/history` 的一个聚合点。 */
type HistoryPoint = {
  ts: number;
  net_rx: number;
  net_tx: number;
  cpu_host: number;
  cpu_proc: number;
};

type RangeKey = '5m' | '10m' | '1h' | '24h' | '7d';

/** 趋势窗口选项。 */
const RANGES: { key: RangeKey; label: string }[] = [
  { key: '5m', label: '5 分钟' },
  { key: '10m', label: '10 分钟' },
  { key: '1h', label: '1 小时' },
  { key: '24h', label: '24 小时' },
  { key: '7d', label: '7 天' },
];

/** 各窗口的历史刷新间隔。窗口越长，刷新越慢。 */
const HISTORY_REFRESH: Record<RangeKey, number> = {
  '5m': 15000,
  '10m': 15000,
  '1h': 60000,
  '24h': 300000,
  '7d': 300000,
};

/** 曲线保留的点数。KPI 的实时速率用。 */
const SAMPLE_LIMIT = 24;
/** 自动刷新间隔。 */
const REFRESH_MS = 5000;
/** 在线频道卡最多列出几个频道。 */
const CHANNEL_ROWS = 12;

/** 模式中文名。与分组管理、订阅源共用 `modes.ts` 的同一来源。 */
function modeLabel(mode: string): string {
  return MODES.find((item) => item.value === mode)?.label ?? mode;
}

/** 字节数转可读文本。 */
function bytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let size = value;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    size /= 1024;
    index += 1;
  }
  // 不足 1 单位时保留一位小数，纵轴刻度不会都取整成同一个数。
  if (index === 0 && !Number.isInteger(size)) return `${size.toFixed(1)} ${units[0]}`;
  return `${size >= 100 || index === 0 ? Math.round(size) : size.toFixed(2)} ${units[index]}`;
}

/** 秒数转「N 小时 M 分钟」。 */
export function uptime(secs: number): string {
  const total = Math.max(0, Math.floor(secs));
  const days = Math.floor(total / 86400);
  const hours = Math.floor((total % 86400) / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  if (days > 0) return `${days} 天 ${hours} 小时`;
  if (hours > 0) return `${hours} 小时 ${minutes} 分钟`;
  return `${minutes} 分钟`;
}

/** 速率的可读文本。 */
function rate(value: number): string {
  return `${bytes(value)}/s`;
}

/** 百分比：保留一位小数，并夹在 0–100。 */
function percent(value: number): number {
  return Math.min(100, Math.max(0, Number.isFinite(value) ? value : 0));
}

function num(source: Status | null, key: string): number {
  const value = source?.[key];
  return typeof value === 'number' && Number.isFinite(value) ? value : 0;
}

function text(source: Status | null, key: string): string {
  const value = source?.[key];
  return value === undefined || value === null ? '—' : String(value);
}

/** 切到时间部分：`2026-11-12 20:00:00` → `11-12 20:00`。 */
function shortTime(value: string): string {
  const match = /^(\d{4})-(\d{2})-(\d{2})[ T](\d{2}):(\d{2})/.exec(value);
  return match ? `${match[2]}-${match[3]} ${match[4]}:${match[5]}` : value || '—';
}

/** 顶部指标卡。色彩只做浅色底，不抢主内容。 */
function Kpi({
  tone,
  icon,
  label,
  value,
  unit,
  sub,
}: {
  tone: 'blue' | 'green' | 'violet' | 'amber';
  icon: IconName;
  label: string;
  value: string;
  unit?: string;
  sub: React.ReactNode;
}) {
  return (
    <div className={`ov-kpi ${tone}`}>
      <div className="ov-kpi-head">
        <span className="ov-kpi-label">{label}</span>
        <span className="ov-kpi-icon">
          <Icon name={icon} size={15} />
        </span>
      </div>
      <div className="ov-kpi-value">
        {value}
        {unit ? <small>{unit}</small> : null}
      </div>
      <div className="ov-kpi-sub">{sub}</div>
    </div>
  );
}

/** 卡内小指标。 */
function Mini({
  label,
  value,
  unit,
  icon,
}: {
  label: string;
  value: string;
  unit?: string;
  icon: IconName;
}) {
  return (
    <div className="ov-mini">
      <div className="ov-mini-label">
        <Icon name={icon} size={12} />
        {label}
      </div>
      <div className="ov-mini-value">
        {value}
        {unit ? <small>{unit}</small> : null}
      </div>
    </div>
  );
}

/** 卡头：标题、说明与右侧操作。 */
function CardHead({
  icon,
  title,
  note,
  children,
}: {
  icon: IconName;
  title: string;
  note?: React.ReactNode;
  children?: React.ReactNode;
}) {
  return (
    <div className="card-head ov-card-head">
      <div className="card-head-text">
        <h3>
          <Icon name={icon} size={14} />
          {title}
        </h3>
        {note ? <span className="muted">{note}</span> : null}
      </div>
      {children ? <div className="card-actions">{children}</div> : null}
    </div>
  );
}

/** 曲线序列。`format` 用于悬停提示里的显式数值。 */
type SparkSeries = {
  label: string;
  color: string;
  values: number[];
  format: (value: number) => string;
};

/** 时钟文本：`HH:MM:SS`。 */
function clockText(ms: number): string {
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`;
}

/** 时间轴标签。短窗口只显示时分，长窗口带日期。 */
function axisTime(ms: number, range: RangeKey): string {
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, '0');
  const day = `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  if (range === '7d') return day;
  if (range === '24h') return `${day} ${time}`;
  return time;
}

/** 悬停提示的时间。短窗口到秒，长窗口到分。 */
function tipTime(ms: number, range: RangeKey): string {
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, '0');
  const day = `${pad(date.getMonth() + 1)}-${pad(date.getDate())}`;
  const time = `${pad(date.getHours())}:${pad(date.getMinutes())}`;
  if (range === '7d') return `${day} ${time}`;
  if (range === '24h') return `${day} ${time}:${pad(date.getSeconds())}`;
  return clockText(ms);
}

/** 纵轴上限取整到 1、2、5 × 10ⁿ，刻度不出现零碎小数。 */
function niceMax(value: number): number {
  if (!(value > 0)) return 1;
  const exponent = Math.floor(Math.log10(value));
  const base = 10 ** exponent;
  const normalized = value / base;
  const step = normalized <= 1 ? 1 : normalized <= 2 ? 2 : normalized <= 5 ? 5 : 10;
  return step * base;
}

/** 折线 + 浅色面积。悬停显示最近采样点的数值。 */
function Sparkline({
  series,
  times,
  max,
  timeFormat,
  height = 112,
}: {
  series: SparkSeries[];
  times: number[];
  /** 纵轴上限。图表卡用它画刻度，曲线用同一个值。 */
  max: number;
  /** 悬停提示的时间格式。 */
  timeFormat: (ms: number) => string;
  height?: number;
}) {
  const rawId = useId();
  const gradientId = `ov-spark-${rawId.replace(/:/g, '')}`;
  const [hover, setHover] = useState<number | null>(null);
  const width = 320;
  const count = times.length;
  // 按时间比例放点：采样缺口不会被压缩成等距数据。
  const start = count > 0 ? times[0] : 0;
  const end = count > 0 ? times[count - 1] : 0;
  const span = Math.max(1, end - start);
  const xAt = (index: number) =>
    count <= 1 ? 0 : ((times[index] - start) / span) * width;
  const yAt = (value: number) => height - 6 - (value / max) * (height - 16);

  /** 指针位置换算成时间最近的数据点。 */
  const onPointerMove = (event: React.PointerEvent<HTMLDivElement>) => {
    if (count === 0) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const ratio = Math.min(1, Math.max(0, (event.clientX - rect.left) / rect.width));
    const target = start + ratio * span;
    let nearest = 0;
    let distance = Number.POSITIVE_INFINITY;
    for (let index = 0; index < count; index += 1) {
      const delta = Math.abs(times[index] - target);
      if (delta < distance) {
        distance = delta;
        nearest = index;
      }
    }
    setHover(nearest);
  };

  const active = hover !== null && hover >= 0 && hover < count ? hover : null;
  const activeRatio = active === null || count <= 1 ? 0 : active / (count - 1);
  // 靠近左右边缘时改变对齐方向，提示框不会越出卡片。
  const tipShift = activeRatio < 0.2 ? '0' : activeRatio > 0.8 ? '-100%' : '-50%';

  return (
    <div
      className="ov-spark-wrap"
      onPointerMove={onPointerMove}
      onPointerLeave={() => setHover(null)}
    >
      <svg
        className="ov-spark"
        viewBox={`0 0 ${width} ${height}`}
        preserveAspectRatio="none"
        role="img"
        aria-label="最近两分钟的采样曲线"
      >
        <defs>
          {series.map((item, index) => (
            <linearGradient
              key={`${gradientId}-${index}`}
              id={`${gradientId}-${index}`}
              x1="0"
              y1="0"
              x2="0"
              y2="1"
            >
              <stop offset="0%" stopColor={item.color} stopOpacity="0.22" />
              <stop offset="100%" stopColor={item.color} stopOpacity="0" />
            </linearGradient>
          ))}
        </defs>
        {[0.25, 0.5, 0.75].map((ratio) => (
          <line
            key={ratio}
            x1="0"
            x2={width}
            y1={height * ratio}
            y2={height * ratio}
            className="ov-spark-grid"
          />
        ))}
        {series.map((item, index) => {
          const points = item.values
            .map((value, at) => `${xAt(at)},${yAt(value)}`)
            .join(' ');
          if (item.values.length < 2) return null;
          const area = `M0,${height} L${points.replaceAll(' ', ' L')} L${width},${height} Z`;
          return (
            <g key={index}>
              <path d={area} fill={`url(#${gradientId}-${index})`} stroke="none" />
              <polyline
                points={points}
                fill="none"
                stroke={item.color}
                strokeWidth="1.8"
                strokeLinecap="round"
                strokeLinejoin="round"
                vectorEffect="non-scaling-stroke"
              />
            </g>
          );
        })}
        {active !== null ? (
          <line
            className="ov-spark-cursor"
            x1={xAt(active)}
            x2={xAt(active)}
            y1="0"
            y2={height}
            vectorEffect="non-scaling-stroke"
          />
        ) : null}
      </svg>
      {active !== null ? (
        <>
          {series.map((item) => {
            const value = item.values[active];
            if (value === undefined) return null;
            return (
              <span
                key={item.label}
                className="ov-spark-dot"
                style={{
                  left: `${activeRatio * 100}%`,
                  top: `${(yAt(value) / height) * 100}%`,
                  background: item.color,
                }}
              />
            );
          })}
          <div
            className="ov-tip"
            style={{
              left: `${activeRatio * 100}%`,
              transform: `translateX(${tipShift})`,
            }}
          >
            <div className="ov-tip-time">{timeFormat(times[active])}</div>
            {series.map((item) => (
              <div className="ov-tip-row" key={item.label}>
                <i style={{ background: item.color }} />
                {item.label}
                <b>{item.format(item.values[active] ?? 0)}</b>
              </div>
            ))}
          </div>
        </>
      ) : null}
    </div>
  );
}

/** 图表卡。数据不足时显示加载或空态提示。 */
function ChartCard({
  title,
  icon,
  note,
  legend,
  series,
  times,
  ready,
  loading,
  emptyText,
  axisFormat,
  tipFormat,
}: {
  title: string;
  icon: IconName;
  note: string;
  legend: { label: string; value: string; color: string }[];
  series: SparkSeries[];
  times: number[];
  ready: boolean;
  loading?: boolean;
  emptyText: string;
  axisFormat: (ms: number) => string;
  tipFormat: (ms: number) => string;
}) {
  // 纵轴上限与曲线共用。序列单位一致，刻度取第一条的单位文本。
  const max = niceMax(Math.max(0, ...series.flatMap((item) => item.values)));
  const tick = series[0]?.format ?? ((value: number) => String(Math.round(value)));
  return (
    <section className="card ov-chart">
      <CardHead icon={icon} title={title} note={note}>
        <div className="ov-legend">
          {legend.map((item) => (
            <span key={item.label} className="ov-legend-item">
              <i style={{ background: item.color }} />
              {item.label}
              <b>{item.value}</b>
            </span>
          ))}
        </div>
      </CardHead>
      {loading ? (
        <div className="ov-empty">加载中…</div>
      ) : ready ? (
        <div className="ov-chart-frame">
          <div className="ov-y-axis">
            <span>{tick(max)}</span>
            <span>{tick(max / 2)}</span>
            <span>{tick(0)}</span>
          </div>
          <Sparkline
            series={series}
            times={times}
            max={max}
            timeFormat={tipFormat}
          />
          <div className="ov-x-axis">
            <span>{axisFormat(times[0])}</span>
            <span>
              {axisFormat(times[0] + (times[times.length - 1] - times[0]) / 2)}
            </span>
            <span>{axisFormat(times[times.length - 1])}</span>
          </div>
        </div>
      ) : (
        <div className="ov-empty">{emptyText}</div>
      )}
    </section>
  );
}

/** 资源卡：标题、占比、进度条与底部明细。 */
function Resource({
  icon,
  title,
  value,
  percentValue,
  hint,
  rows,
  tone,
}: {
  icon: IconName;
  title: string;
  value: string;
  percentValue: number | null;
  hint?: string;
  rows: React.ReactNode[];
  tone: 'blue' | 'green' | 'violet';
}) {
  return (
    <section className="card ov-res">
      <CardHead icon={icon} title={title}>
        <span className="ov-res-pct">
          {percentValue === null ? '—' : `${percentValue.toFixed(1)}%`}
        </span>
      </CardHead>
      <div className="ov-res-value">{value}</div>
      {percentValue === null ? (
        <div className="ov-bar empty" aria-hidden="true" />
      ) : (
        <div
          className={`ov-bar ${tone}`}
          role="progressbar"
          aria-label={`${title} 占用`}
          aria-valuemin={0}
          aria-valuemax={100}
          aria-valuenow={Math.round(percentValue)}
        >
          <span style={{ width: `${percentValue}%` }} />
        </div>
      )}
      {hint ? <div className="hint">{hint}</div> : null}
      <div className="ov-res-foot">
        {rows.map((row, index) => (
          <div key={index}>{row}</div>
        ))}
      </div>
    </section>
  );
}

export function Overview({
  features,
  featureLabels,
}: {
  features: Record<string, boolean> | null;
  featureLabels: [string, string][];
}) {
  const [status, setStatus] = useState<Status | null>(null);
  const [stats, setStats] = useState<PlayStats | null>(null);
  const [tasks, setTasks] = useState<Task[] | null>(null);
  const [dash, setDash] = useState<DashSession[]>([]);
  const [samples, setSamples] = useState<Sample[]>([]);
  const [range, setRange] = useState<RangeKey>('5m');
  const [history, setHistory] = useState<HistoryPoint[] | null>(null);
  const [historyError, setHistoryError] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState('');
  const { node, ok, err } = useToast();

  /** 记一次采样。重启后计数回退时重置曲线，避免断崖。
   *  两次采样相隔不足 2 秒时替换最后一点，避免除以极小间隔得到虚假尖峰。 */
  const sample = useCallback((body: Status) => {
    const next: Sample = {
      at: Date.now(),
      out: num(body, 'traffic_out_bytes'),
      cpu: num(body, 'cpu_seconds'),
    };
    setSamples((prev) => {
      const last = prev[prev.length - 1];
      if (last && (next.out < last.out || next.cpu < last.cpu)) return [next];
      if (last && next.at - last.at < 2000) {
        return [...prev.slice(0, -1), next].slice(-SAMPLE_LIMIT);
      }
      return [...prev, next].slice(-SAMPLE_LIMIT);
    });
  }, []);

  // 功能开关以 `/status` 为准：切回概览后，新开启的分区立即开始拉数据。
  // 只用两个布尔量做依赖，避免 status 每次轮询都重建 load。
  const liveFeatures =
    (status?.features as Record<string, boolean> | undefined) ?? features;
  const wantStats = liveFeatures?.stats !== false;
  const wantRecording = liveFeatures?.recording !== false;

  /** 最新一次刷新请求的序号。旧响应晚到时直接丢弃，避免回退计数触发重启分支。 */
  const loadTicket = useRef(0);
  const load = useCallback(
    async (manual = false) => {
      const ticket = loadTicket.current + 1;
      loadTicket.current = ticket;
      if (manual) setBusy('refresh');
      try {
        const [nextStatus, nextStats, nextTasks, nextDash] = await Promise.all([
          api<Status>('GET', '/status'),
          wantStats
            ? api<PlayStats>('GET', '/stats').catch(() => null)
            : Promise.resolve(null),
          wantRecording
            ? api<Task[]>('GET', '/schedule').catch(() => null)
            : Promise.resolve(null),
          api<DashSession[]>('GET', '/dash/status').catch(() => []),
        ]);
        if (ticket !== loadTicket.current) return;
        setStatus(nextStatus);
        setStats(nextStats);
        setTasks(Array.isArray(nextTasks) ? nextTasks : null);
        setDash(Array.isArray(nextDash) ? nextDash : []);
        setError('');
        sample(nextStatus);
      } catch (cause) {
        if (ticket !== loadTicket.current) return;
        setError(cause instanceof Error ? cause.message : String(cause));
      } finally {
        if (manual) setBusy('');
      }
    },
    [wantStats, wantRecording, sample],
  );

  // 首屏加载，之后按固定间隔刷新。页面隐藏时跳过，省请求。
  useEffect(() => {
    void load();
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void load();
    }, REFRESH_MS);
    return () => window.clearInterval(timer);
  }, [load]);

  /** 读取趋势窗口的历史点。切换窗口时清空旧数据。
   *  用请求序号丢弃过期响应，快速切换窗口不会显示上一个窗口的数据。 */
  const historyTicket = useRef(0);
  const loadHistory = useCallback(async (next: RangeKey) => {
    const ticket = historyTicket.current + 1;
    historyTicket.current = ticket;
    try {
      const body = await api<{ points?: HistoryPoint[] }>(
        'GET',
        `/metrics/history?range=${next}`,
      );
      if (ticket !== historyTicket.current) return;
      setHistory(Array.isArray(body.points) ? body.points : []);
      setHistoryError('');
    } catch (cause) {
      if (ticket !== historyTicket.current) return;
      setHistory([]);
      setHistoryError(cause instanceof Error ? cause.message : String(cause));
    }
  }, []);

  // 窗口变化时重新拉取。窗口越长，刷新间隔越长。
  useEffect(() => {
    setHistory(null);
    void loadHistory(range);
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') void loadHistory(range);
    }, HISTORY_REFRESH[range]);
    return () => window.clearInterval(timer);
  }, [range, loadHistory]);

  /** 快速操作：成功后刷新状态。 */
  const action = async (name: string, path: string, done: string) => {
    setBusy(name);
    try {
      await api('POST', path);
      ok(done);
      await load();
    } catch (cause) {
      err(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy('');
    }
  };

  const go = (section: string) => {
    window.dispatchEvent(new CustomEvent(`tv-buddy:goto-${section}`));
  };

  const uptimeSecs = num(status, 'uptime_secs');
  const groupCount = num(status, 'group_count');
  const channelCount = num(status, 'channel_count');
  const onlineViewers = stats?.online_viewers ?? num(status, 'online_viewers');
  const activeChannels = stats?.active_channels ?? 0;
  const windowSecs = stats?.online_window_secs ?? 0;
  const outBytes = num(status, 'traffic_out_bytes');
  const cores = Math.max(1, num(status, 'cpu_cores'));
  const hostCpu = percent(num(status, 'host_cpu_percent'));
  const rssMb = num(status, 'mem_rss_mb');
  const memTotalMb = num(status, 'mem_total_mb');
  const cacheMb = num(status, 'disk_cache_mb');
  const recordingMb = num(status, 'recording_mb');
  const diskTotalMb = num(status, 'disk_total_mb');
  const diskFreeMb = num(status, 'disk_free_mb');
  const loadAvg = Array.isArray(status?.load_avg)
    ? (status.load_avg as unknown[]).map((value) =>
        typeof value === 'number' ? value.toFixed(2) : '0.00',
      )
    : ['0.00', '0.00', '0.00'];
  const groupModes = (status?.group_modes ?? {}) as Record<string, number>;
  const cacheBackend = text(status, 'cache_backend') === 'memory' ? '内存' : '硬盘';
  const version = status?.version ? `v${status.version}` : '—';
  const bind = text(status, 'bind');
  const cachePath = text(status, 'cache_disk_path');

  // 相邻采样求差：应用出网与进程 CPU。趋势图改用后端历史，不再用这里。
  const rates = useMemo(() => {
    const list: { rate: number; cpu: number }[] = [];
    for (let index = 1; index < samples.length; index += 1) {
      const previous = samples[index - 1];
      const current = samples[index];
      const seconds = (current.at - previous.at) / 1000;
      // 间隔过短的采样会让速率虚高，直接跳过。
      if (seconds < 0.5) continue;
      const outDelta = Math.max(0, current.out - previous.out);
      const cpuDelta = Math.max(0, current.cpu - previous.cpu);
      list.push({
        rate: outDelta / seconds,
        cpu: (cpuDelta / seconds / cores) * 100,
      });
    }
    return list;
  }, [samples, cores]);

  const lastRate = rates.length > 0 ? rates[rates.length - 1] : null;
  const procCpu = percent(lastRate?.cpu ?? 0);
  const memPercent =
    memTotalMb > 0 ? percent((rssMb / memTotalMb) * 100) : null;
  const diskUsedMb = cacheMb + recordingMb;
  // 存储卡看整个文件系统：缓存只是其中一部分，单独占比没有参考价值。
  const diskFsUsedMb = Math.max(0, diskTotalMb - diskFreeMb);
  const diskPercent =
    diskTotalMb > 0 ? percent((diskFsUsedMb / diskTotalMb) * 100) : null;
  // 趋势图的数据来自历史接口。少于两点画不出线，按空态处理。
  const points = history ?? [];
  const historyTimes = points.map((point) => point.ts * 1000);
  const lastPoint = points.length > 0 ? points[points.length - 1] : null;
  const historyReady = points.length >= 2;

  // 与侧栏同一口径：缺少键也算启用，只有显式 false 才隐藏。
  const enabledFeatures = liveFeatures
    ? featureLabels.filter(([key]) => liveFeatures[key] !== false)
    : [];
  const statsEnabled = wantStats;
  const recordingEnabled = wantRecording;
  const playChannels = stats?.channels ?? [];
  const pendingTasks = (tasks ?? []).filter((task) => task.status === 'pending');
  const upcoming = [...pendingTasks].sort((a, b) =>
    a.start_time.localeCompare(b.start_time),
  );

  return (
    <>
      <PageHead
        icon="overview"
        title="系统概览"
        sub="实时运行状态、播放流量与资源占用"
        sticky
      >
        <span className="badge ok">● 运行 {uptime(uptimeSecs)}</span>
        {status !== null && groupCount > 0 ? (
          <>
            <button
              className="ghost"
              title="删除已保存的清单与切片，释放磁盘"
              disabled={busy !== ''}
              onClick={() => void action('flush', '/cache/flush', '本地缓存已清空')}
            >
              <Icon name="trash" size={14} />
              {busy === 'flush' ? '清理中…' : '清空缓存'}
            </button>
            <button
              className="ghost"
              title="从数据库重新读取配置并立即生效"
              disabled={busy !== ''}
              onClick={() => void action('reload', '/reload', '配置已重新读取')}
            >
              <Icon name="refresh" size={14} />
              {busy === 'reload' ? '读取中…' : '重读配置'}
            </button>
          </>
        ) : null}
        <button
          className="ghost"
          title="立即刷新状态与实时指标"
          disabled={busy !== ''}
          onClick={() => void load(true)}
        >
          <Icon name="refresh" size={14} />
          {busy === 'refresh' ? '刷新中…' : '刷新'}
        </button>
      </PageHead>

      <div className="ov-kpis">
        <Kpi
          tone="blue"
          icon="livestream"
          label="频道总数"
          value={String(channelCount)}
          unit="个"
          sub={`${groupCount} 个分组 · ${dash.length} 个 DASH 会话`}
        />
        <Kpi
          tone="green"
          icon="users"
          label="在线观众"
          value={String(onlineViewers)}
          unit="人"
          sub={
            !statsEnabled
              ? '频道热度未启用'
              : stats
                ? `${activeChannels} 个频道在播 · ${windowSecs} 秒窗口`
                : '正在读取统计数据'
          }
        />
        <Kpi
          tone="violet"
          icon="network"
          label="本次出网"
          value={bytes(outBytes)}
          sub={`当前 ${rate(lastRate?.rate ?? 0)} · 缓存 ${bytes(cacheMb * 1024 * 1024)}`}
        />
        <Kpi
          tone="amber"
          icon="schedule"
          label="运行时长"
          value={uptime(uptimeSecs)}
          sub={`${version} · 监听 ${bind}`}
        />
      </div>

      {status !== null && groupCount === 0 ? (
        <section className="card setup-card">
          <div>
            <h2>从第一个分组开始</h2>
            <p className="muted">
              创建分组并添加频道，或用 M3U 导入。保存后订阅地址会立即更新。
            </p>
          </div>
          <button className="ghost" onClick={() => go('groups')}>
            <Icon name="groups" size={14} />
            打开分组管理
          </button>
        </section>
      ) : null}

      <section className="card ov-play">
        <CardHead
          icon="play"
          title="播放流量概览"
          note="当前播放按最近一段时间内的活跃客户端统计"
        >
          {statsEnabled ? (
            <button className="ghost" onClick={() => go('stats')}>
              详细统计
              <Icon name="up" size={13} className="ov-arrow" />
            </button>
          ) : null}
          <button className="ghost" onClick={() => go('my')}>
            <Icon name="subs" size={14} />
            订阅地址
          </button>
        </CardHead>
        <div className="ov-mini-row">
          <Mini label="当前播放" value={String(onlineViewers)} unit="人" icon="users" />
          <Mini label="活跃频道" value={String(activeChannels)} unit="个" icon="activity" />
          <Mini label="本次流量" value={bytes(outBytes)} icon="upload" />
          <Mini label="当前速率" value={rate(lastRate?.rate ?? 0)} icon="network" />
        </div>
        {!statsEnabled ? (
          <div className="ov-empty">频道热度未启用，开启后显示在线频道明细</div>
        ) : playChannels.length === 0 ? (
          <div className="ov-empty">本次服务运行期间暂无播放流量</div>
        ) : null}
      </section>

      {recordingEnabled ? (
        <section className="card ov-tasks">
          <CardHead
            icon="schedule"
            title="定时录制"
            note={`${(tasks ?? []).length} 个任务 · ${pendingTasks.length} 个待触发`}
          >
            <button className="ghost" onClick={() => go('schedule')}>
              查看全部
              <Icon name="up" size={13} className="ov-arrow" />
            </button>
          </CardHead>
          {tasks === null ? (
            <div className="ov-empty">加载中…</div>
          ) : upcoming.length > 0 ? (
            <div className="ov-task-list">
              {upcoming.slice(0, 4).map((task) => (
                <div className="ov-task" key={task.id}>
                  <span className="ov-task-name">{task.name || task.id}</span>
                  <span className="muted mono">{shortTime(task.start_time)}</span>
                  <span className={`badge ${STATUS_TONE[task.status] ?? 'muted'}`}>
                    {STATUS_LABELS[task.status] ?? task.status}
                  </span>
                </div>
              ))}
            </div>
          ) : (
            <div className="ov-empty">
              暂无待触发任务。
              <button className="ov-link" onClick={() => go('schedule')}>
                新建任务 →
              </button>
            </div>
          )}
        </section>
      ) : null}

      <div className="ov-res-grid">
        <Resource
          icon="cpu"
          title="CPU"
          tone="blue"
          value={`${hostCpu.toFixed(1)}%`}
          percentValue={hostCpu}
          hint={
            memTotalMb > 0 ? undefined : '主机指标不可用，仅显示本进程数据'
          }
          rows={[
            `进程 ${procCpu.toFixed(1)}% · ${cores} 核`,
            `负载 ${loadAvg.join(' / ')}`,
          ]}
        />
        <Resource
          icon="memory"
          title="内存"
          tone="green"
          value={
            memTotalMb > 0
              ? `${bytes(rssMb * 1024 * 1024)} / ${bytes(memTotalMb * 1024 * 1024)}`
              : bytes(rssMb * 1024 * 1024)
          }
          percentValue={memPercent}
          hint={memTotalMb > 0 ? undefined : '读不到主机内存总量'}
          rows={[
            `进程 RSS ${bytes(rssMb * 1024 * 1024)}`,
            `整盘内存预算 ${bytes(num(status, 'whole_file_budget'))}`,
          ]}
        />
        <Resource
          icon="disk"
          title="存储"
          tone="violet"
          value={
            diskTotalMb > 0
              ? `${bytes(diskFsUsedMb * 1024 * 1024)} / ${bytes(diskTotalMb * 1024 * 1024)}`
              : bytes(diskUsedMb * 1024 * 1024)
          }
          percentValue={diskPercent}
          hint={diskTotalMb > 0 ? undefined : '读不到磁盘容量'}
          rows={[
            `缓存 ${bytes(cacheMb * 1024 * 1024)} · 录制 ${bytes(recordingMb * 1024 * 1024)}`,
            <>
              可用 {diskTotalMb > 0 ? bytes(diskFreeMb * 1024 * 1024) : '—'} ·{' '}
              <span className="mono">{cachePath}</span>
            </>,
          ]}
        />
      </div>

      <div className="ov-chart-bar">
        <span className="muted">趋势窗口</span>
        <div className="ov-range" role="group" aria-label="趋势时间窗口">
          {RANGES.map((item) => (
            <button
              key={item.key}
              className={`ov-range-btn${range === item.key ? ' active' : ''}`}
              aria-pressed={range === item.key}
              onClick={() => setRange(item.key)}
            >
              {item.label}
            </button>
          ))}
        </div>
      </div>

      <div className="ov-chart-grid">
        <ChartCard
          icon="network"
          title="网络吞吐"
          note="主机网卡收发速率"
          legend={[
            { label: '入', value: rate(lastPoint?.net_rx ?? 0), color: 'var(--chart-b)' },
            { label: '出', value: rate(lastPoint?.net_tx ?? 0), color: 'var(--chart-a)' },
          ]}
          series={[
            {
              label: '入',
              color: 'var(--chart-b)',
              values: points.map((point) => point.net_rx),
              format: rate,
            },
            {
              label: '出',
              color: 'var(--chart-a)',
              values: points.map((point) => point.net_tx),
              format: rate,
            },
          ]}
          times={historyTimes}
          ready={historyReady}
          loading={history === null}
          emptyText={historyError || '该窗口暂无数据'}
          axisFormat={(ms) => axisTime(ms, range)}
          tipFormat={(ms) => tipTime(ms, range)}
        />
        <ChartCard
          icon="activity"
          title="CPU 负载"
          note="主机与本进程 CPU 占用"
          legend={[
            {
              label: '主机',
              value: `${(lastPoint?.cpu_host ?? 0).toFixed(1)}%`,
              color: 'var(--chart-c)',
            },
            {
              label: '进程',
              value: `${(lastPoint?.cpu_proc ?? 0).toFixed(1)}%`,
              color: 'var(--chart-b)',
            },
          ]}
          series={[
            {
              label: '主机',
              color: 'var(--chart-c)',
              values: points.map((point) => point.cpu_host),
              format: (value) => `${value.toFixed(1)}%`,
            },
            {
              label: '进程',
              color: 'var(--chart-b)',
              values: points.map((point) => point.cpu_proc),
              format: (value) => `${value.toFixed(1)}%`,
            },
          ]}
          times={historyTimes}
          ready={historyReady}
          loading={history === null}
          emptyText={historyError || '该窗口暂无数据'}
          axisFormat={(ms) => axisTime(ms, range)}
          tipFormat={(ms) => tipTime(ms, range)}
        />
      </div>

      <div className="dash-grid">
        <div className="dash-main">
          <section className="card ov-dash">
            <CardHead
              icon="livestream"
              title="在线频道"
              note="最近窗口内有人观看的频道"
            >
              {statsEnabled ? (
                <button className="ghost" onClick={() => go('stats')}>
                  查看全部
                  <Icon name="up" size={13} className="ov-arrow" />
                </button>
              ) : null}
            </CardHead>
            {!statsEnabled ? (
              <div className="ov-empty">频道热度未启用</div>
            ) : playChannels.length === 0 ? (
              <div className="ov-empty">当前没有在线频道</div>
            ) : (
              <div className="ov-channel-list">
                {playChannels.slice(0, CHANNEL_ROWS).map((channel) => (
                  <div className="ov-channel" key={`${channel.gid}/${channel.cid}`}>
                    <span className="ov-channel-name">
                      {channel.name || `${channel.gid}/${channel.cid}`}
                    </span>
                    <span className="muted mono">{channel.online} 人</span>
                    <span className="muted mono">{channel.requests} 次</span>
                  </div>
                ))}
              </div>
            )}
          </section>
        </div>

        <div className="dash-rail">
          <section className="card ov-info">
            <CardHead icon="server" title="系统信息" />
            <div className="kv-list">
              <div className="kv">
                <span>版本</span>
                <b>{version}</b>
              </div>
              <div className="kv">
                <span>运行时长</span>
                <b>{uptime(uptimeSecs)}</b>
              </div>
              <div className="kv">
                <span>缓存位置</span>
                <b className="mono">{cacheBackend}</b>
              </div>
              <div className="kv">
                <span>监听地址</span>
                <b className="mono">{bind}</b>
              </div>
              <div className="kv">
                <span>面板会话</span>
                <b>{num(status, 'active_sessions')}</b>
              </div>
            </div>
            <div className="feature-block">
              <div className="feature-title">已启用功能</div>
              <div className="feature-list">
                {enabledFeatures.length === 0 ? (
                  <span className="muted">{liveFeatures ? '全部关闭' : '—'}</span>
                ) : (
                  enabledFeatures.map(([key, label]) => (
                    <span key={key} className="badge muted">
                      {label}
                    </span>
                  ))
                )}
              </div>
            </div>
          </section>

          <section className="card ov-dist">
            <CardHead
              icon="groups"
              title="分组分布"
              note="按工作模式统计分组数"
            />
            <div className="ov-dist-list">
              <div className="ov-dist-row">
                <span>分组</span>
                <b>{groupCount}</b>
              </div>
              <div className="ov-dist-row">
                <span>频道</span>
                <b>{channelCount}</b>
              </div>
              {Object.entries(groupModes).map(([mode, count]) => (
                <div className="ov-dist-row" key={mode}>
                  <span>
                    <i className={`ov-dot ${mode}`} />
                    {modeLabel(mode)}
                  </span>
                  <b>{count}</b>
                </div>
              ))}
              <div className="ov-dist-row">
                <span>在线频道</span>
                <b>{stats ? activeChannels : '—'}</b>
              </div>
              <div className="ov-dist-row">
                <span>DASH 会话</span>
                <b>{dash.length}</b>
              </div>
            </div>
          </section>
        </div>
      </div>

      <div className="overview-system">
        <SystemPage embedded />
      </div>

      {error && <div className="msg err">{error}</div>}
      {node}
    </>
  );
}
