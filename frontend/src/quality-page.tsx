/** 质量校验页：概览、规则、结果与来源。
 *
 * 契约见 `docs/design/16-quality-admission-system.md` §8–§9。
 * 结果页与来源页在后续任务补齐筛选与操作，这里先给出可读视图。
 */

import { useCallback, useEffect, useMemo, useState } from 'react';

import {
  api,
  cancelQuality,
  fetchQualityResults,
  fetchQualitySources,
  fetchQualityStatus,
  previewQualitySchedule,
  putConfig,
  runQuality,
  type QualityResult,
  type QualitySource,
  type QualityStatus,
} from './api';
import { Icon } from './icons';
import { QualityBadges } from './quality';
import { Badge, Card, Empty, Field, PageHead, Select, Switch, useToast } from './ui';

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
};

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

const OVERRIDE_LABEL: Record<string, string> = {
  enabled: '开关',
  min_speed_mib: '最低速率',
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
function RulesTab({ onSaved }: { onSaved: () => void }) {
  const { node, ok, err } = useToast();
  const [draft, setDraft] = useState<Cfg | null>(null);
  const [busy, setBusy] = useState('');
  const [preview, setPreview] = useState<number[]>([]);
  const [previewError, setPreviewError] = useState('');

  const load = useCallback(async () => {
    try {
      const config = await api<Cfg>('GET', '/config');
      setDraft(config);
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
            <input
              type="number"
              min={1}
              max={8760}
              value={asNumber(draft, 'quality.interval_hours')}
              onChange={(event) =>
                update('quality.interval_hours', Number(event.target.value) || 1)
              }
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
        </div>
        {previewError ? <div className="hint">{previewError}</div> : null}
        {preview.length > 0 ? (
          <div className="hint">未来运行：{preview.map(timeText).join('；')}</div>
        ) : null}
      </div>
      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>判定阈值</h2>
            <span className="muted">频道、分组、来源的覆盖优先于这里的全局值</span>
          </div>
        </div>
        <div className="fields">
          <Field label="最低速率" hint="MiB/s，0 = 不限制">
            <input
              type="number"
              min={0}
              step={0.1}
              value={asNumber(draft, 'quality.rules.min_speed_mib')}
              onChange={(event) =>
                update('quality.rules.min_speed_mib', Number(event.target.value) || 0)
              }
            />
          </Field>
          <Field label="最大延迟" hint="毫秒，0 = 不限制">
            <input
              type="number"
              min={0}
              value={asNumber(draft, 'quality.rules.max_delay_ms')}
              onChange={(event) =>
                update('quality.rules.max_delay_ms', Number(event.target.value) || 0)
              }
            />
          </Field>
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
          <Field label="兜底保留" tip="有连通但阈值不合格时仍保留。">
            <Switch
              checked={asBool(draft, 'quality.rules.supply')}
              onChange={(next) => update('quality.rules.supply', next)}
              label="兜底保留"
            />
          </Field>
          <Field label="Host 缓存" tip="复用同一 Host 的成功测速，减少重复下载。">
            <Switch
              checked={asBool(draft, 'quality.rules.host_cache')}
              onChange={(next) => update('quality.rules.host_cache', next)}
              label="Host 缓存"
            />
          </Field>
        </div>
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
            <input
              type="number"
              min={0.01}
              max={1}
              step={0.01}
              value={asNumber(draft, 'quality.max_remove_ratio')}
              onChange={(event) =>
                update('quality.max_remove_ratio', Number(event.target.value) || 0.5)
              }
            />
          </Field>
          <Field label="最小移出个数" hint="移出数量低于该值不触发">
            <input
              type="number"
              min={1}
              value={asNumber(draft, 'quality.min_remove_count')}
              onChange={(event) =>
                update('quality.min_remove_count', Number(event.target.value) || 1)
              }
            />
          </Field>
          <Field label="结果保留天数" hint="0 = 不自动清理">
            <input
              type="number"
              min={0}
              max={3650}
              value={asNumber(draft, 'quality.result_retention_days')}
              onChange={(event) =>
                update('quality.result_retention_days', Number(event.target.value) || 0)
              }
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
function ResultsTab() {
  const [items, setItems] = useState<QualityResult[] | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    fetchQualityResults({ limit: 50 })
      .then((body) => {
        if (active) setItems(body.items ?? []);
      })
      .catch((err) => {
        if (active) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      active = false;
    };
  }, []);

  if (error) return <Empty text={error} icon="activity" />;
  if (!items) return <Empty text="正在读取结果…" icon="activity" />;
  if (items.length === 0) return <Empty text="还没有质量结果" icon="activity" />;

  return (
    <div className="card">
      <table className="table">
        <thead>
          <tr>
            <th>频道</th>
            <th>来源</th>
            <th>动作</th>
            <th>原因</th>
            <th>质量</th>
            <th>校验时间</th>
          </tr>
        </thead>
        <tbody>
          {items.map((item) => (
            <tr key={item.key}>
              <td>{item.name || item.cid}</td>
              <td>{item.source_id || '手工'}</td>
              <td>{ACTION_LABEL[item.action] ?? item.action}</td>
              <td>{REASON_LABEL[item.reason] ?? item.reason}</td>
              <td>
                <QualityBadges result={item} />
              </td>
              <td>{timeText(item.checked_at)}</td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** 来源标签。 */
function SourcesTab() {
  const [items, setItems] = useState<QualitySource[] | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    let active = true;
    fetchQualitySources()
      .then((body) => {
        if (active) setItems(body.items ?? []);
      })
      .catch((err) => {
        if (active) setError(err instanceof Error ? err.message : String(err));
      });
    return () => {
      active = false;
    };
  }, []);

  if (error) return <Empty text={error} icon="subs" />;
  if (!items) return <Empty text="正在读取来源…" icon="subs" />;
  if (items.length === 0) return <Empty text="没有同步来源" icon="subs" />;

  return (
    <div className="card">
      <table className="table">
        <thead>
          <tr>
            <th>来源</th>
            <th>启用</th>
            <th>门控</th>
            <th>阶段</th>
            <th>校验</th>
            <th>准入</th>
            <th>拒绝</th>
            <th>移出</th>
            <th>剩余候选</th>
          </tr>
        </thead>
        <tbody>
          {items.map((item) => (
            <tr key={item.id}>
              <td>{item.name || item.id}</td>
              <td>
                <Badge tone={item.enabled ? 'ok' : 'muted'}>
                  {item.enabled ? '启用' : '停用'}
                </Badge>
              </td>
              <td>
                <Badge tone={item.gate_enabled ? 'ok' : 'muted'}>
                  {item.gate_enabled ? '开' : '关'}
                </Badge>
              </td>
              <td>{item.phase || '—'}</td>
              <td>{item.tested}</td>
              <td>{item.admitted}</td>
              <td>{item.rejected}</td>
              <td>{item.removed}</td>
              <td>
                {item.remaining}
                {item.paused ? <Badge tone="warn">暂停</Badge> : null}
                {item.truncated ? <Badge tone="warn">截断</Badge> : null}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}

/** 质量校验页。 */
export function QualityPage() {
  const { node, ok, err } = useToast();
  const [tab, setTab] = useState<Tab>('overview');
  const [status, setStatus] = useState<QualityStatus | null>(null);
  const [busy, setBusy] = useState('');
  const [confirmCancel, setConfirmCancel] = useState(false);

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
      setTab('overview');
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
      ok('已请求取消');
      await load();
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
        <button onClick={() => void run()} disabled={busy !== '' || status?.running}>
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
            onClick={() => setTab(id)}
          >
            {label}
          </button>
        ))}
      </div>
      {tab === 'overview' && (
        <OverviewTab
          status={status}
          busy={busy !== ''}
          onRun={() => void run()}
          onCancel={() => setConfirmCancel(true)}
        />
      )}
      {tab === 'rules' && <RulesTab onSaved={() => void load()} />}
      {tab === 'results' && <ResultsTab />}
      {tab === 'sources' && <SourcesTab />}
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
