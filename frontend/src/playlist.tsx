/** 订阅源：聚合与托管共同形成一个总清单。 */

import { useCallback, useEffect, useMemo, useState } from 'react';

import { api, ApiError } from './api';
import { Badge, DateInput, Drawer, Field, Modal, NumberBox, PageHead, Select, Tip, useToast } from './ui';

type Source = {
  id: string;
  name: string;
  url: string;
  enabled: boolean;
  ttl_secs: number;
  group_mode: string;
  merge_group: string;
  unlisted_groups: string;
  group_map: Record<string, string>;
  include: string[];
  exclude: string[];
  mode: string;
  schedule: string;
  sync_interval_secs: number;
  sync_daily_at: string;
  group_prefix: string;
  playback_mode: string;
  format: string;
  position: string;
  headers: string[];
  proxy?: {
    kind: string;
    addr: string;
    username?: string | null;
    password?: string | null;
  } | null;
  max_channels: number;
  merge_into: Record<string, string>;
};

type GlobalCfg = {
  merge_ttl_secs: number;
  merge_position: string;
  global_exclude: string[];
  sync_enabled: boolean;
  sync_schedule: string;
  sync_interval_secs: number;
  sync_daily_at: string;
  sync_allow_empty: boolean;
  sync_warn_removal_ratio: number;
  sync_max_channels: number;
  sync_total_warn: number;
  sync_concurrency: number;
  sync_fetch_timeout_secs: number;
};

type Status = {
  id: string;
  mode: string;
  error: string | null;
  channels: number;
  groups: number;
  last_ok: number;
  last_try: number;
  last_sync: number;
  added: number;
  updated: number;
  removed: number;
  skipped_removals: number;
  needs_review: boolean;
  zero_blocked: boolean;
  shrink_permille: number;
  duration_ms: number;
  syncing: boolean;
  next_sync: number;
};

type GroupCount = { name: string; count: number };
type ExistingGroup = { id: string; name: string; origin?: string };

type SyncPreview = {
  add: number;
  update: number;
  remove: number;
  groups_add: number;
  groups_remove: number;
  skipped_duplicates: number;
  guard: {
    over_limit: boolean;
    empty: boolean;
    max_channels: number;
    allow_empty: boolean;
  };
};

type Preview = {
  kept: number;
  sample: { name: string; group: string; tvg_id: string }[];
  sync?: SyncPreview | null;
  trace: {
    total: number;
    dropped_groups: string[];
    out_groups: GroupCount[];
  };
};

type DrawerState = {
  isNew: boolean;
  draft: Source;
  groups: GroupCount[] | null;
  preview: Preview | null;
  error: string;
  busy: boolean;
};

type Pending =
  | { kind: 'save'; source: Source; id: string; channels: number }
  | { kind: 'delete'; id: string; name: string; channels: number };

const GLOBAL_DEFAULT: GlobalCfg = {
  merge_ttl_secs: 300,
  merge_position: 'bottom',
  global_exclude: [],
  sync_enabled: true,
  sync_schedule: 'interval',
  sync_interval_secs: 21600,
  sync_daily_at: '03:30',
  sync_allow_empty: false,
  sync_warn_removal_ratio: 0.2,
  sync_max_channels: 20000,
  sync_total_warn: 5000,
  sync_concurrency: 1,
  sync_fetch_timeout_secs: 25,
};

function blankSource(): Source {
  return {
    id: '',
    name: '',
    url: '',
    enabled: true,
    ttl_secs: 0,
    group_mode: 'keep',
    merge_group: '',
    unlisted_groups: 'drop',
    group_map: {},
    include: [],
    exclude: [],
    mode: 'aggregate',
    schedule: 'inherit',
    sync_interval_secs: 0,
    sync_daily_at: '',
    group_prefix: '',
    playback_mode: 'auto',
    format: 'auto',
    position: '',
    headers: [],
    proxy: null,
    max_channels: 0,
    merge_into: {},
  };
}

function lines(value: string): string[] {
  return value
    .split('\n')
    .map((line) => line.trim())
    .filter(Boolean);
}

function mapText(map: Record<string, string>): string {
  return Object.entries(map)
    .map(([from, to]) => `${from}=${to}`)
    .join('\n');
}

function parseMap(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const line of lines(text)) {
    const index = line.indexOf('=');
    if (index <= 0) continue;
    out[line.slice(0, index).trim()] = line.slice(index + 1).trim();
  }
  return out;
}

/** 代理字段 → 草稿值。保留空地址，保存时再清空。 */
function proxyDraft(
  current: Source['proxy'],
  patch: Partial<NonNullable<Source['proxy']>>,
): Source['proxy'] {
  return {
    kind: patch.kind ?? current?.kind ?? 'http',
    addr: patch.addr ?? current?.addr ?? '',
    username: patch.username ?? current?.username,
    password: patch.password ?? current?.password,
  };
}

/** 保存前清掉空地址代理，避免每轮抓取都告警。 */
function cleanSource(source: Source): Source {
  const proxy = source.proxy?.addr.trim() ? source.proxy : null;
  return { ...source, proxy };
}

function when(value: number): string {
  if (!value) return '—';
  return new Date(value * 1000).toLocaleString();
}

function sourceModeBadge(source: Source) {
  return source.mode === 'sync' ? (
    <Badge tone="info">托管</Badge>
  ) : (
    <Badge tone="muted">聚合</Badge>
  );
}

function statusBadge(status?: Status, warnRatio = 0.2) {
  if (!status) return null;
  if (status.syncing) return <Badge tone="info">同步中</Badge>;
  if (status.needs_review) return <Badge tone="warn">待确认</Badge>;
  if (status.error) return <Badge tone="danger">失败</Badge>;
  const threshold = Math.max(0, Math.round(warnRatio * 1000));
  if (status.shrink_permille > 0 && status.shrink_permille >= threshold) {
    return <Badge tone="warn">缩水</Badge>;
  }
  if (status.zero_blocked) return <Badge tone="warn">空清单</Badge>;
  return <Badge tone="ok">正常</Badge>;
}

export function PlaylistPage() {
  const [global, setGlobal] = useState<GlobalCfg>(GLOBAL_DEFAULT);
  const [sources, setSources] = useState<Source[]>([]);
  const [statuses, setStatuses] = useState<Record<string, Status>>({});
  const [drawer, setDrawer] = useState<DrawerState | null>(null);
  const [pending, setPending] = useState<Pending | null>(null);
  const [allGroups, setAllGroups] = useState<ExistingGroup[]>([]);
  const [unauthorizedSync, setUnauthorizedSync] = useState(0);
  const { node, ok, err } = useToast();

  const load = useCallback(async () => {
    try {
      const next = await api<GlobalCfg & { sources?: Partial<Source>[] }>('GET', '/playlist');
      setGlobal({ ...GLOBAL_DEFAULT, ...next });
      setSources((next.sources ?? []).map((source) => ({ ...blankSource(), ...source })));
      const status = await api<{ sources: Status[] }>('GET', '/subs/status').catch(() => ({
        sources: [],
      }));
      setStatuses(Object.fromEntries((status.sources ?? []).map((item) => [item.id, item])));
      // 同步分组不自动授权。统计未授权给受限用户的数量，只做提示。
      const [users, options] = await Promise.all([
        api<{ users: { role: string; allowed_group_ids: string[] }[] }>('GET', '/users').catch(
          () => ({ users: [] }),
        ),
        api<{ groups: ExistingGroup[] }>('GET', '/users/channel-options').catch(() => ({
          groups: [],
        })),
      ]);
      const restricted = (users.users ?? []).filter((user) => user.role !== 'admin');
      if (restricted.length === 0) {
        setUnauthorizedSync(0);
      } else {
        const granted = new Set(restricted.flatMap((user) => user.allowed_group_ids ?? []));
        setUnauthorizedSync(
          (options.groups ?? []).filter((group) => group.origin === 'sync' && !granted.has(group.id))
            .length,
        );
      }
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  const loadGroups = useCallback(async () => {
    try {
      const config = await api<{ groups?: ExistingGroup[] }>('GET', '/config');
      setAllGroups(config.groups ?? []);
    } catch {
      // 分组列表只用于并入选择器；读取失败不影响编辑。
    }
  }, []);

  useEffect(() => {
    void load();
    void loadGroups();
  }, [load, loadGroups]);

  // 同步状态轮询：托管源在后台跑，页面需要看到进度。
  useEffect(() => {
    const timer = window.setInterval(() => {
      void api<{ sources: Status[] }>('GET', '/subs/status')
        .then((status) =>
          setStatuses(Object.fromEntries((status.sources ?? []).map((item) => [item.id, item]))),
        )
        .catch(() => undefined);
    }, 5000);
    return () => window.clearInterval(timer);
  }, []);

  const saveGlobal = async () => {
    try {
      await api('PUT', '/playlist/settings', global);
      ok('全局设置已保存');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const openSource = (source?: Source) => {
    const draft = source ? { ...blankSource(), ...source } : blankSource();
    setDrawer({
      isNew: !source,
      draft,
      groups: null,
      preview: null,
      error: '',
      busy: false,
    });
  };

  const patch = (value: Partial<Source>) => {
    setDrawer((current) =>
      current ? { ...current, draft: { ...current.draft, ...value }, preview: null } : current,
    );
  };

  const readGroups = async () => {
    if (!drawer) return;
    const url = drawer.draft.url.trim();
    if (!url) {
      err('先填写源地址');
      return;
    }
    try {
      const data = await api<{ groups: GroupCount[]; total: number }>('POST', '/subs/groups', {
        id: drawer.draft.id || undefined,
        url,
        mode: drawer.draft.mode,
        headers: drawer.draft.headers,
        proxy: drawer.draft.proxy?.addr.trim() ? drawer.draft.proxy : undefined,
      });
      setDrawer((current) => (current ? { ...current, groups: data.groups ?? [] } : current));
      ok(`读到 ${(data.groups ?? []).length} 个分组、${data.total} 个频道`);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const runPreview = async () => {
    if (!drawer) return;
    const source = drawer.draft;
    if (!source.url.trim()) {
      err('先填写源地址');
      return;
    }
    setDrawer((current) => (current ? { ...current, busy: true, error: '' } : current));
    try {
      const result = await api<Preview>('POST', '/subs/preview', {
        ...cleanSource(source),
        id: source.id || undefined,
      });
      setDrawer((current) =>
        current ? { ...current, preview: result, busy: false } : current,
      );
    } catch (error) {
      const text = error instanceof Error ? error.message : String(error);
      setDrawer((current) =>
        current ? { ...current, busy: false, error: text, preview: null } : current,
      );
    }
  };

  const saveSource = async (source: Source, keepContent?: boolean | null, thenSync = false) => {
    const wasSync = source.id
      ? sources.find((item) => item.id === source.id)?.mode === 'sync'
      : false;
    const query =
      keepContent === undefined || keepContent === null
        ? ''
        : `?keep_content=${keepContent ? 1 : 0}`;
    try {
      const payload = cleanSource(source);
      let saved: Source;
      if (!source.id) {
        const result = await api<{ source: Source }>('POST', '/playlist/sources', payload);
        saved = result.source;
      } else {
        const result = await api<{ source: Source }>(
          'PUT',
          `/playlist/sources/${encodeURIComponent(source.id)}${query}`,
          payload,
        );
        saved = result.source;
      }
      ok(source.mode === 'sync' ? '托管源已保存' : '聚合源已保存');
      setDrawer(null);
      setPending(null);
      await load();
      await loadGroups();
      if (thenSync && saved.id && saved.mode === 'sync') {
        await triggerSync(saved.id);
      } else if (!wasSync && saved.id && saved.mode === 'sync' && saved.enabled) {
        // 聚合切托管：立即补一次同步，不等下一个周期。禁用源交给调度器跳过。
        await triggerSync(saved.id);
      }
    } catch (error) {
      if (error instanceof ApiError && error.code === 'content_exists') {
        const channels = statuses[source.id]?.channels ?? 0;
        setDrawer((current) => (current ? { ...current, busy: false } : current));
        setPending({ kind: 'save', source, id: source.id, channels });
        return;
      }
      const text = error instanceof Error ? error.message : String(error);
      setDrawer((current) => (current ? { ...current, busy: false, error: text } : current));
      err(text);
    }
  };

  const submitDrawer = (thenSync: boolean) => {
    if (!drawer) return;
    if (!drawer.draft.name.trim() || !drawer.draft.url.trim()) {
      setDrawer({ ...drawer, error: '名称与地址不能为空' });
      return;
    }
    setDrawer({ ...drawer, busy: true, error: '' });
    void saveSource(drawer.draft, undefined, thenSync);
  };

  const triggerSync = async (id: string) => {
    try {
      await api('POST', '/subs/sync', { id });
      ok('已排入同步队列');
      window.setTimeout(() => void load(), 1200);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const askDelete = async (source: Source) => {
    try {
      await api('DELETE', `/playlist/sources/${encodeURIComponent(source.id)}`);
      ok('源已删除，内容保留为手工');
      await load();
    } catch (error) {
      if (error instanceof ApiError && error.code === 'content_exists') {
        setPending({
          kind: 'delete',
          id: source.id,
          name: source.name,
          channels: statuses[source.id]?.channels ?? 0,
        });
        return;
      }
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const resolveDelete = async (toManual: boolean) => {
    if (!pending || pending.kind !== 'delete') return;
    try {
      await api(
        'DELETE',
        `/playlist/sources/${encodeURIComponent(pending.id)}?keep_content=${toManual ? 1 : 0}`,
      );
      ok(toManual ? '源已删除，内容转为手工' : '源与内容已删除');
      setPending(null);
      setDrawer(null);
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const mergedGroupOptions = useMemo(
    () => [
      { value: '', label: '独立建组（默认）' },
      ...allGroups.map((group) => ({
        value: group.id,
        label: `${group.name}（${group.id}）`,
      })),
    ],
    [allGroups],
  );

  const totalSyncChannels = sources.reduce(
    (sum, source) =>
      source.mode === 'sync' ? sum + (statuses[source.id]?.channels ?? 0) : sum,
    0,
  );

  return (
    <>
      <PageHead
        icon="subs"
        title="订阅源"
        sub="聚合与托管共同形成一个总清单。托管源按计划同步进分组与频道。"
      >
        <button onClick={() => openSource()}>添加源</button>
        <Tip text="聚合源在渲染订阅时拼接；托管源会物化进总清单，可管理、可授权。" />
      </PageHead>

      {unauthorizedSync > 0 ? (
        <div className="msg warn">
          有 {unauthorizedSync} 个同步分组未授权给受限用户。请在「用户与设备」页勾选。
        </div>
      ) : null}
      {global.sync_total_warn > 0 && totalSyncChannels > global.sync_total_warn ? (
        <div className="msg warn">
          托管频道共 {totalSyncChannels} 条，超过建议阈值 {global.sync_total_warn} 条。大清单会增大配置写入时间。
        </div>
      ) : null}

      <div className="card">
        <div className="row between">
          <h2>订阅源</h2>
          <span className="muted">{sources.length} 个源</span>
        </div>
        <div className="table-wrap">
          <table className="table">
            <thead>
              <tr>
                <th>启用</th>
                <th>名称</th>
                <th>地址</th>
                <th>模式</th>
                <th className="right">频道 / 分组</th>
                <th>最近同步</th>
                <th>状态</th>
                <th className="right">操作</th>
              </tr>
            </thead>
            <tbody>
              {sources.map((source) => {
                const status = statuses[source.id];
                return (
                  <tr key={source.id}>
                    <td>
                      <input
                        type="checkbox"
                        aria-label={`启用 ${source.name}`}
                        checked={source.enabled}
                        onChange={(event) =>
                          void saveSource({ ...source, enabled: event.target.checked })
                        }
                      />
                    </td>
                    <td>
                      <strong>{source.name || source.id}</strong>
                      {status?.error ? (
                        <div className="muted" style={{ fontSize: 12 }}>
                          {status.error}
                        </div>
                      ) : null}
                    </td>
                    <td className="mono" style={{ maxWidth: 320, overflowWrap: 'anywhere' }}>
                      {source.url}
                    </td>
                    <td>{sourceModeBadge(source)}</td>
                    <td className="right">
                      {source.mode === 'sync'
                        ? `${status?.channels ?? 0} / ${status?.groups ?? 0}`
                        : '—'}
                    </td>
                    <td>{when(status?.last_sync ?? 0)}</td>
                    <td>{statusBadge(status, global.sync_warn_removal_ratio)}</td>
                    <td className="right">
                      {source.mode === 'sync' && (
                        <button
                          className="ghost"
                          title="立即同步这个源"
                          disabled={status?.syncing}
                          onClick={() => void triggerSync(source.id)}
                        >
                          立即同步
                        </button>
                      )}
                      <button className="ghost" onClick={() => openSource(source)}>
                        编辑
                      </button>
                      <button className="chip danger-text" onClick={() => void askDelete(source)}>
                        删除
                      </button>
                    </td>
                  </tr>
                );
              })}
              {sources.length === 0 && (
                <tr>
                  <td colSpan={8} className="muted">
                    暂无订阅源。点右上角「添加源」开始。
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </div>
      </div>

      <div className="card">
        <div className="row between">
          <h2>全局设置</h2>
          <button onClick={() => void saveGlobal()}>保存全局设置</button>
        </div>
        <div className="row">
          <label>
            聚合缓存秒数
            <Tip text="聚合结果缓存多久。缓存期内直接复用上次结果。" />
            <NumberBox
              value={global.merge_ttl_secs}
              onChange={(next) =>
                setGlobal({ ...global, merge_ttl_secs: Number(next) || 300 })
              }
            />
          </label>
          <Field label="合并位置" tip="聚合条目插在每个用户订阅的顶部或底部。">
            <Select
              ariaLabel="合并位置"
              value={global.merge_position}
              options={[
                { value: 'bottom', label: '底部' },
                { value: 'top', label: '顶部' },
              ]}
              onChange={(next) => setGlobal({ ...global, merge_position: next })}
              compact
            />
          </Field>
          <label style={{ alignSelf: 'end' }}>
            <input
              type="checkbox"
              checked={global.sync_enabled}
              onChange={(event) =>
                setGlobal({ ...global, sync_enabled: event.target.checked })
              }
            />{' '}
            自动同步
            <Tip text="关闭后仍可在列表里手动触发同步。" />
          </label>
        </div>
        <div className="row">
          <Field label="默认调度">
            <Select
              ariaLabel="默认调度"
              value={global.sync_schedule}
              options={[
                { value: 'interval', label: '按间隔' },
                { value: 'daily', label: '每天定时' },
              ]}
              onChange={(next) => setGlobal({ ...global, sync_schedule: next })}
              compact
            />
          </Field>
          <label>
            默认间隔（秒）
            <NumberBox
              min={300}
              value={global.sync_interval_secs}
              onChange={(next) =>
                setGlobal({
                  ...global,
                  sync_interval_secs: Number(next) || 21600,
                })
              }
            />
          </label>
          <label>
            默认每天时间
            <DateInput
              type="time"
              ariaLabel="默认每天时间"
              value={global.sync_daily_at}
              onChange={(next) => setGlobal({ ...global, sync_daily_at: next })}
            />
          </label>
        </div>
        <div className="row">
          <label>
            单源频道上限
            <NumberBox
              min={0}
              value={global.sync_max_channels}
              onChange={(next) =>
                setGlobal({ ...global, sync_max_channels: Number(next) || 0 })
              }
            />
          </label>
          <label>
            总频道警告阈值
            <NumberBox
              min={0}
              value={global.sync_total_warn}
              onChange={(next) =>
                setGlobal({ ...global, sync_total_warn: Number(next) || 0 })
              }
            />
          </label>
          <label>
            缩水标红比例
            <NumberBox
              min={0}
              max={1}
              step={0.05}
              value={global.sync_warn_removal_ratio}
              onChange={(next) =>
                setGlobal({
                  ...global,
                  sync_warn_removal_ratio: Number(next) || 0,
                })
              }
            />
          </label>
          <label style={{ alignSelf: 'end' }}>
            <input
              type="checkbox"
              checked={global.sync_allow_empty}
              onChange={(event) =>
                setGlobal({ ...global, sync_allow_empty: event.target.checked })
              }
            />{' '}
            允许空清单清空
            <Tip text="关闭时，源返回 0 条不会清空已有内容，只标记待确认。" />
          </label>
        </div>
        <label>
          全局排除
          <Tip text="匹配的频道不会出现在任何聚合或同步结果里。每行一条。" />
          <textarea
            aria-label="全局排除关键词"
            value={(global.global_exclude ?? []).join('\n')}
            onChange={(event) =>
              setGlobal({ ...global, global_exclude: lines(event.target.value) })
            }
            spellCheck={false}
          />
        </label>
      </div>

      {drawer && (
        <Drawer
          title={drawer.isNew ? '添加订阅源' : `编辑：${drawer.draft.name || drawer.draft.id}`}
          onClose={() => setDrawer(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setDrawer(null)}>
                取消
              </button>
              <button disabled={drawer.busy} onClick={() => submitDrawer(false)}>
                保存
              </button>
              {drawer.draft.mode === 'sync' && (
                <button disabled={drawer.busy} onClick={() => submitDrawer(true)}>
                  保存并开始同步
                </button>
              )}
            </>
          }
        >
          {drawer.error && <p className="msg danger">{drawer.error}</p>}
          <h3>基本</h3>
          <div className="row">
            <label>
              名称
              <input
                value={drawer.draft.name}
                onChange={(event) => patch({ name: event.target.value })}
              />
            </label>
            <label style={{ alignSelf: 'end' }}>
              <input
                type="checkbox"
                checked={drawer.draft.enabled}
                onChange={(event) => patch({ enabled: event.target.checked })}
              />{' '}
              启用
            </label>
          </div>
          <label>
            地址
            <input
              value={drawer.draft.url}
              placeholder="http(s)://…/playlist.m3u"
              onChange={(event) => patch({ url: event.target.value })}
            />
          </label>
          <hr className="divider" />
          <h3>模式</h3>
          <Field
            label="处理方式"
            tip="聚合：渲染订阅时拼接。托管：物化进总清单，可管理、可授权、进 EPG。"
          >
            <Select
              ariaLabel="处理方式"
              value={drawer.draft.mode}
              options={[
                { value: 'aggregate', label: '聚合（渲染时拼接）' },
                { value: 'sync', label: '托管（物化进总清单）' },
              ]}
              onChange={(next) => patch({ mode: next })}
            />
          </Field>
          {drawer.draft.mode === 'sync' && (
            <p className="muted">
              托管源会按分组建立真实分组。删除源或改回聚合时，系统会先询问内容的去留。
            </p>
          )}
          <hr className="divider" />
          <h3>分组</h3>
          <div className="row">
            <Field label="分组模式" tip="keep 保留原名；map 按映射改名；merge 合并到同一组。">
              <Select
                ariaLabel="分组模式"
                value={drawer.draft.group_mode}
                options={[
                  { value: 'keep', label: 'keep' },
                  { value: 'map', label: 'map' },
                  { value: 'merge', label: 'merge' },
                ]}
                onChange={(next) => patch({ group_mode: next })}
                compact
              />
            </Field>
            {drawer.draft.group_mode === 'merge' && (
              <label>
                合并到分组
                <Tip text="保留的上游分组都并入该名称。未列出分组要设为 keep，或填写分组映射。" />
                <input
                  value={drawer.draft.merge_group}
                  placeholder="目标分组名"
                  onChange={(event) => patch({ merge_group: event.target.value })}
                />
              </label>
            )}
            <Field label="未列出分组" tip="没有出现在映射里的分组保留还是丢弃。">
              <Select
                ariaLabel="未列出分组"
                value={drawer.draft.unlisted_groups}
                options={[
                  { value: 'drop', label: 'drop' },
                  { value: 'keep', label: 'keep' },
                ]}
                onChange={(next) => patch({ unlisted_groups: next })}
                compact
              />
            </Field>
            <label>
              分组名前缀
              <Tip text="留空时用「源名 · 」。" />
              <input
                value={drawer.draft.group_prefix}
                placeholder={drawer.draft.name ? `${drawer.draft.name} · ` : ''}
                onChange={(event) => patch({ group_prefix: event.target.value })}
              />
            </label>
          </div>
          <div className="row">
            <button className="ghost" disabled={drawer.busy} onClick={() => void readGroups()}>
              读取分组
            </button>
            <Tip text="只读取源的分组名与条数，不修改配置。" />
            {drawer.draft.mode === 'sync' &&
              drawer.draft.unlisted_groups === 'drop' &&
              Object.keys(drawer.draft.group_map).length === 0 && (
                <span className="msg warn">
                  {drawer.draft.group_mode === 'merge'
                    ? '合并模式：未列出分组为 drop 且没有映射，不会导入任何频道。请把「未列出分组」改为 keep。'
                    : '没有分组映射且未列出分组为 drop：不会导入任何频道。'}
                </span>
              )}
          </div>
          {drawer.groups && drawer.groups.length > 0 && (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>上游分组</th>
                    <th className="right">频道</th>
                    <th>并入已有分组</th>
                    <th className="right">操作</th>
                  </tr>
                </thead>
                <tbody>
                  {drawer.groups.map((group) => (
                    <tr key={group.name}>
                      <td>{group.name}</td>
                      <td className="right">{group.count}</td>
                      <td>
                        {drawer.draft.mode === 'sync' && (
                          <Select
                            ariaLabel={`${group.name} 并入分组`}
                            value={drawer.draft.merge_into[group.name] ?? ''}
                            options={mergedGroupOptions}
                            onChange={(next) => {
                              const mergeInto = { ...drawer.draft.merge_into };
                              if (next) mergeInto[group.name] = next;
                              else delete mergeInto[group.name];
                              patch({ merge_into: mergeInto });
                            }}
                            compact
                          />
                        )}
                      </td>
                      <td className="right">
                        <button
                          className="ghost"
                          onClick={() =>
                            patch({
                              group_map: {
                                ...drawer.draft.group_map,
                                [group.name]:
                                  drawer.draft.group_map[group.name] ?? group.name,
                              },
                            })
                          }
                        >
                          加入映射
                        </button>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
          <label>
            分组映射
            <Tip text="每行一条「上游分组=目标分组」。分组模式为 map 时生效。" />
            <textarea
              value={mapText(drawer.draft.group_map)}
              onChange={(event) => patch({ group_map: parseMap(event.target.value) })}
            />
          </label>
          <div className="row">
            <label>
              include
              <Tip text="只保留匹配的行。支持 group:、id:、url: 前缀。" />
              <textarea
                value={drawer.draft.include.join('\n')}
                onChange={(event) => patch({ include: lines(event.target.value) })}
              />
            </label>
            <label>
              exclude
              <Tip text="匹配的频道会被排除；删除同步频道时会自动写入。" />
              <textarea
                value={drawer.draft.exclude.join('\n')}
                onChange={(event) => patch({ exclude: lines(event.target.value) })}
              />
            </label>
          </div>

          {drawer.draft.mode === 'sync' && (
            <>
              <hr className="divider" />
              <h3>同步</h3>
              <div className="row">
                <Field label="调度">
                  <Select
                    ariaLabel="调度"
                    value={drawer.draft.schedule}
                    options={[
                      { value: 'inherit', label: '继承全局' },
                      { value: 'interval', label: '按间隔' },
                      { value: 'daily', label: '每天定时' },
                    ]}
                    onChange={(next) => patch({ schedule: next })}
                    compact
                  />
                </Field>
                <label>
                  间隔（秒）
                  <NumberBox
                    min={0}
                    value={drawer.draft.sync_interval_secs}
                    onChange={(next) => patch({ sync_interval_secs: Number(next) || 0 })}
                  />
                  <span className="hint">0 继承全局</span>
                </label>
                <label>
                  每天时间
                  <DateInput
                    type="time"
                    ariaLabel="每天时间"
                    value={drawer.draft.sync_daily_at}
                    onChange={(next) => patch({ sync_daily_at: next })}
                  />
                  <span className="hint">留空继承全局</span>
                </label>
              </div>
              <div className="row">
                <Field label="播放模式" tip="托管源禁止 passthrough。auto 按地址后缀选择。">
                  <Select
                    ariaLabel="播放模式"
                    value={drawer.draft.playback_mode}
                    options={[
                      { value: 'auto', label: 'auto' },
                      { value: 'proxy', label: 'proxy' },
                      { value: 'remux', label: 'remux' },
                      { value: 'dash', label: 'dash' },
                      { value: 'redirect', label: 'redirect' },
                      { value: 'rewrite', label: 'rewrite' },
                    ]}
                    onChange={(next) => patch({ playback_mode: next })}
                    compact
                  />
                </Field>
                <Field label="格式">
                  <Select
                    ariaLabel="格式"
                    value={drawer.draft.format}
                    options={[
                      { value: 'auto', label: 'auto' },
                      { value: 'hls', label: 'hls' },
                      { value: 'mpd', label: 'mpd' },
                    ]}
                    onChange={(next) => patch({ format: next })}
                    compact
                  />
                </Field>
                <Field label="插入位置">
                  <Select
                    ariaLabel="插入位置"
                    value={drawer.draft.position}
                    options={[
                      { value: '', label: '继承全局' },
                      { value: 'top', label: '顶部' },
                      { value: 'bottom', label: '底部' },
                    ]}
                    onChange={(next) => patch({ position: next })}
                    compact
                  />
                </Field>
                <label>
                  单源上限
                  <NumberBox
                    min={0}
                    value={drawer.draft.max_channels}
                    onChange={(next) => patch({ max_channels: Number(next) || 0 })}
                  />
                  <span className="hint">0 继承全局</span>
                </label>
              </div>
            </>
          )}

          <hr className="divider" />
          <h3>高级</h3>
          <label>
            抓取请求头
            <Tip text="每行一条「名称: 值」。只用于抓取清单本身。" />
            <textarea
              value={drawer.draft.headers.join('\n')}
              onChange={(event) => patch({ headers: lines(event.target.value) })}
            />
          </label>
          <div className="row">
            <label>
              代理类型
              <input
                value={drawer.draft.proxy?.kind ?? ''}
                placeholder="http"
                onChange={(event) =>
                  patch({
                    proxy: proxyDraft(drawer.draft.proxy, { kind: event.target.value }),
                  })
                }
              />
            </label>
            <label>
              代理地址
              <input
                value={drawer.draft.proxy?.addr ?? ''}
                placeholder="host:port"
                onChange={(event) =>
                  patch({
                    proxy: proxyDraft(drawer.draft.proxy, { addr: event.target.value }),
                  })
                }
              />
            </label>
          </div>
          <div className="row">
            <label>
              代理用户名
              <input
                value={drawer.draft.proxy?.username ?? ''}
                autoComplete="off"
                onChange={(event) =>
                  patch({
                    proxy: proxyDraft(drawer.draft.proxy, { username: event.target.value }),
                  })
                }
              />
            </label>
            <label>
              代理密码
              <input
                type="password"
                value={drawer.draft.proxy?.password ?? ''}
                autoComplete="new-password"
                onChange={(event) =>
                  patch({
                    proxy: proxyDraft(drawer.draft.proxy, { password: event.target.value }),
                  })
                }
              />
            </label>
          </div>

          <hr className="divider" />
          <h3>预检</h3>
          <div className="row">
            <button className="ghost" disabled={drawer.busy} onClick={() => void runPreview()}>
              抓取并预检
            </button>
            <Tip text="读取源、按规则过滤，并列出托管模式的差异。不写配置。" />
          </div>
          {drawer.preview && (
            <>
              <p className="muted">
                共 {drawer.preview.trace.total} 条，保留 {drawer.preview.kept} 条。
              </p>
              {drawer.preview.sync && (
                <div className="row">
                  <Badge tone="ok">新增 {drawer.preview.sync.add}</Badge>
                  <Badge tone="info">更新 {drawer.preview.sync.update}</Badge>
                  <Badge tone="warn">移除 {drawer.preview.sync.remove}</Badge>
                  <Badge tone="muted">重复 {drawer.preview.sync.skipped_duplicates}</Badge>
                  {drawer.preview.sync.guard.over_limit && (
                    <Badge tone="danger">超过单源上限</Badge>
                  )}
                  {drawer.preview.sync.guard.empty && (
                    <Badge tone="warn">空清单会拦截</Badge>
                  )}
                </div>
              )}
              <div className="table-wrap">
                <table className="table">
                  <thead>
                    <tr>
                      <th>频道</th>
                      <th>分组</th>
                      <th>tvg-id</th>
                    </tr>
                  </thead>
                  <tbody>
                    {drawer.preview.sample.map((item, index) => (
                      <tr key={`${item.name}-${index}`}>
                        <td>{item.name}</td>
                        <td>{item.group}</td>
                        <td className="mono">{item.tvg_id || '—'}</td>
                      </tr>
                    ))}
                  </tbody>
                </table>
              </div>
            </>
          )}
        </Drawer>
      )}

      {pending && (
        <Modal
          title="这个源已有内容"
          onClose={() => setPending(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setPending(null)}>
                取消
              </button>
              <button
                className="ghost"
                onClick={() => {
                  if (pending.kind === 'delete') void resolveDelete(true);
                  else void saveSource(pending.source, true, false);
                }}
              >
                保留为手工
              </button>
              <button
                onClick={() => {
                  if (pending.kind === 'delete') void resolveDelete(false);
                  else void saveSource(pending.source, false, false);
                }}
              >
                一并删除
              </button>
            </>
          }
        >
          <p>
            {pending.kind === 'delete'
              ? `「${pending.name}」已有 ${pending.channels} 个频道。`
              : `「${pending.source.name}」已有 ${pending.channels} 个频道。切换为聚合后，这些内容不再跟随上游。`}
          </p>
          <p className="muted">保留为手工：内容与分组保留，来源关系断开。</p>
          <p className="muted">一并删除：删除该源创建的分组与频道。</p>
        </Modal>
      )}

      {node}
    </>
  );
}
