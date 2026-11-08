/** 订阅源：聚合与托管共同形成一个总清单。 */

import { useCallback, useEffect, useMemo, useState } from 'react';

import { api, ApiError } from './api';
import {
  Badge,
  DateInput,
  Drawer,
  Field,
  HoverTip,
  Modal,
  NumberBox,
  PageHead,
  Select,
  Tip,
  useToast,
} from './ui';

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
  merge_new: Record<string, string>;
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
  groups_rename: number;
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
    unlisted_groups: 'keep',
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
    merge_new: {},
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

/** 去掉未填完整的目标项，避免把空键或空值发给后端。 */
function cleanTargets(map: Record<string, string>): Record<string, string> {
  return Object.fromEntries(
    Object.entries(map)
      .map(([from, to]) => [from.trim(), to.trim()] as const)
      .filter(([from, to]) => from !== '' && to !== ''),
  );
}

/** 保存前清掉空地址代理，避免每轮抓取都告警。 */
function cleanSource(source: Source): Source {
  const proxy = source.proxy?.addr.trim() ? source.proxy : null;
  return {
    ...source,
    proxy,
    merge_into: cleanTargets(source.merge_into),
    merge_new: cleanTargets(source.merge_new),
  };
}

/** 每一行的目标方式。已选已有组时优先，与后端一致。 */
function targetModeOf(source: Source, name: string): 'default' | 'new' | 'existing' {
  if (Object.prototype.hasOwnProperty.call(source.merge_into, name)) return 'existing';
  if (Object.prototype.hasOwnProperty.call(source.merge_new, name)) return 'new';
  return 'default';
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

type StatusView = { tone: string; label: string; hint: string };

/** 状态徽章的文字、颜色与悬停说明。只算一次，避免两处规则漂移。 */
function statusView(status?: Status, warnRatio = 0.2): StatusView | null {
  if (!status) return null;
  if (status.syncing) {
    return { tone: 'info', label: '同步中', hint: '正在同步这个源。' };
  }
  if (status.error) {
    return {
      tone: 'danger',
      label: '失败',
      hint: status.needs_review
        ? `${status.error} 另外，源返回空清单，本地频道已保留。`
        : status.error,
    };
  }
  if (status.needs_review) {
    return {
      tone: 'warn',
      label: '待确认',
      hint: '源返回空清单。本地频道已保留。如需接受空清单，请开启全局「允许空清单清空」。',
    };
  }
  const threshold = Math.max(0, Math.round(warnRatio * 1000));
  if (status.shrink_permille > 0 && status.shrink_permille >= threshold) {
    return {
      tone: 'warn',
      label: '频道变少',
      hint: `最近一次同步移除了 ${(status.shrink_permille / 10).toFixed(1)}% 的频道。`,
    };
  }
  if (status.zero_blocked) {
    return {
      tone: 'warn',
      label: '空清单',
      hint: '源返回空清单。本地频道已保留。',
    };
  }
  return { tone: 'ok', label: '正常', hint: '' };
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
      current
        ? { ...current, draft: { ...current.draft, ...value }, preview: null, error: '' }
        : current,
    );
  };

  const setTargetMode = (name: string, mode: 'default' | 'new' | 'existing') => {
    if (!drawer) return;
    const mergeInto = { ...drawer.draft.merge_into };
    const mergeNew = { ...drawer.draft.merge_new };
    delete mergeInto[name];
    delete mergeNew[name];
    if (mode === 'existing') mergeInto[name] = '';
    if (mode === 'new') mergeNew[name] = '';
    patch({ merge_into: mergeInto, merge_new: mergeNew });
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
      const nextGroups = data.groups ?? [];
      setDrawer((current) => {
        if (!current) return current;
        // 源分组变化后，清掉已不存在分组的目标项，避免保存时被空值拦住。
        const names = new Set(nextGroups.map((group) => group.name));
        return {
          ...current,
          groups: nextGroups,
          draft: {
            ...current.draft,
            merge_into: Object.fromEntries(
              Object.entries(current.draft.merge_into).filter(([name]) => names.has(name)),
            ),
            merge_new: Object.fromEntries(
              Object.entries(current.draft.merge_new).filter(([name]) => names.has(name)),
            ),
          },
        };
      });
      ok(`读到 ${nextGroups.length} 个分组、${data.total} 个频道`);
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
    if (drawer.draft.mode === 'sync') {
      const badExisting = Object.entries(drawer.draft.merge_into).some(
        ([from, to]) => !from.trim() || !to.trim(),
      );
      if (badExisting) {
        setDrawer({ ...drawer, error: '请为每个「并入已有组」选择目标分组' });
        return;
      }
      const badNew = Object.entries(drawer.draft.merge_new).some(
        ([from, to]) => !from.trim() || !to.trim(),
      );
      if (badNew) {
        setDrawer({ ...drawer, error: '请填写每个「合并到新组」的组名' });
        return;
      }
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
      ok('源已删除，频道保留为本地频道');
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
      ok(toManual ? '源已删除，频道转为本地频道' : '源与频道已删除');
      setPending(null);
      setDrawer(null);
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const existingGroupOptions = useMemo(
    () =>
      allGroups.map((group) => ({
        value: group.id,
        label: `${group.name}（${group.id}）`,
      })),
    [allGroups],
  );

  const totalSyncChannels = sources.reduce(
    (sum, source) =>
      source.mode === 'sync' ? sum + (statuses[source.id]?.channels ?? 0) : sum,
    0,
  );

  // 后端 GroupRules 的可见结果：map 模式 + 空映射 + 丢弃 → 零频道。
  // 显式目标不受「映射外丢弃」影响，所以存在目标时不再提示零频道。
  const draft = drawer?.draft;
  const draftMapEmpty = draft ? Object.keys(draft.group_map).length === 0 : true;
  const hasExplicitTargets =
    draft?.mode === 'sync' &&
    (Object.keys(draft.merge_into).length > 0 || Object.keys(draft.merge_new).length > 0);
  const showUnlistedGroups =
    draft !== undefined &&
    (draft.group_mode === 'map' || !draftMapEmpty || hasExplicitTargets);
  const dropsAllGroups =
    draft !== undefined &&
    draft.group_mode === 'map' &&
    draft.unlisted_groups === 'drop' &&
    draftMapEmpty &&
    !hasExplicitTargets;

  return (
    <>
      <PageHead
        icon="subs"
        title="订阅源"
        sub="聚合源在播放时读取源。托管源把频道保存到本地。"
      >
        <button onClick={() => openSource()}>添加源</button>
        <Tip text="聚合：每次播放时读取源。托管：频道保存在本地，可以分组、授权、设置节目单。" />
      </PageHead>

      {unauthorizedSync > 0 ? (
        <div className="msg warn">
          有 {unauthorizedSync} 个托管分组还没有授权给普通用户。请到「用户与设备」页勾选。
        </div>
      ) : null}
      {global.sync_total_warn > 0 && totalSyncChannels > global.sync_total_warn ? (
        <div className="msg warn">
          托管频道共 {totalSyncChannels} 条，超过建议上限 {global.sync_total_warn} 条。清单太大时，保存配置会变慢。
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
                const view = statusView(status, global.sync_warn_removal_ratio);
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
                    <td>
                      {view ? (
                        <HoverTip text={view.hint}>
                          <Badge tone={view.tone}>{view.label}</Badge>
                        </HoverTip>
                      ) : null}
                    </td>
                    <td className="right">
                      {source.mode === 'sync' && (
                        <button
                          className="ghost"
                          title="马上同步这个源"
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
        <div className="form-grid four">
          <label>
            聚合缓存（秒）
            <Tip text="这段时间内重复读取源时，直接使用上次结果。" />
            <NumberBox
              value={global.merge_ttl_secs}
              onChange={(next) =>
                setGlobal({ ...global, merge_ttl_secs: Number(next) || 300 })
              }
            />
          </label>
          <Field label="合并位置" tip="聚合内容放在每个用户订阅的哪一端。">
            <Select
              ariaLabel="合并位置"
              value={global.merge_position}
              options={[
                { value: 'bottom', label: '底部', desc: '放在订阅末尾。' },
                { value: 'top', label: '顶部', desc: '放在订阅开头。' },
              ]}
              onChange={(next) => setGlobal({ ...global, merge_position: next })}
              compact
            />
          </Field>
          <label className="check form-check">
            <input
              type="checkbox"
              checked={global.sync_enabled}
              onChange={(event) =>
                setGlobal({ ...global, sync_enabled: event.target.checked })
              }
            />
            <span>
              自动同步
              <Tip text="关闭后，托管源不会按计划自动同步。你仍可在列表里手动同步。" />
            </span>
          </label>
        </div>
        <div className="form-grid four">
          <Field label="默认调度" tip="新建托管源默认使用的同步时间。">
            <Select
              ariaLabel="默认调度"
              value={global.sync_schedule}
              options={[
                { value: 'interval', label: '按间隔', desc: '每隔一段时间同步一次。' },
                { value: 'daily', label: '每天定时', desc: '每天在固定时间同步一次。' },
              ]}
              onChange={(next) => setGlobal({ ...global, sync_schedule: next })}
              compact
            />
          </Field>
          {global.sync_schedule === 'daily' ? (
            <label>
              默认每天时间
              <Tip text="每天执行同步的时间。" />
              <DateInput
                type="time"
                ariaLabel="默认每天时间"
                value={global.sync_daily_at}
                onChange={(next) => setGlobal({ ...global, sync_daily_at: next })}
              />
            </label>
          ) : (
            <label>
              默认间隔（秒）
              <Tip text="两次同步之间等待的秒数。最短 300 秒。" />
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
          )}
        </div>
        <div className="form-grid four">
          <label>
            单源频道上限
            <Tip text="一个托管源最多导入多少条频道。填 0 表示不限。" />
            <NumberBox
              min={0}
              value={global.sync_max_channels}
              onChange={(next) =>
                setGlobal({ ...global, sync_max_channels: Number(next) || 0 })
              }
            />
          </label>
          <label>
            总频道提醒上限
            <Tip text="托管频道总数超过这个值时给出提醒。填 0 表示不提醒。" />
            <NumberBox
              min={0}
              value={global.sync_total_warn}
              onChange={(next) =>
                setGlobal({ ...global, sync_total_warn: Number(next) || 0 })
              }
            />
          </label>
          <label>
            缩水提醒比例
            <Tip text="一次同步删除的频道超过这个比例时标黄。例如 0.2 表示 20%。" />
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
          <label className="check form-check">
            <input
              type="checkbox"
              checked={global.sync_allow_empty}
              onChange={(event) =>
                setGlobal({ ...global, sync_allow_empty: event.target.checked })
              }
            />
            <span>
              允许空清单清空
              <Tip text="开启后，源返回 0 条时会清空本地频道。关闭时保留原频道，只标记待确认。" />
            </span>
          </label>
        </div>
        <label>
          全局排除
          <Tip text="这些名字不会出现在聚合和同步结果里。每行一条。支持 *（任意长度）和 ?（一个字符）通配符。" />
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
          <div className="form-grid two">
            <label>
              名称
              <input
                value={drawer.draft.name}
                onChange={(event) => patch({ name: event.target.value })}
              />
            </label>
            <label className="check form-check">
              <input
                type="checkbox"
                checked={drawer.draft.enabled}
                onChange={(event) => patch({ enabled: event.target.checked })}
              />
              <span>
                启用
                <Tip text="关闭后，这个源不参与聚合，也不自动同步。" />
              </span>
            </label>
          </div>
          <label>
            地址
            <Tip text="M3U 播放列表的地址。支持 http(s) 链接。" />
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
            tip="选择这个源的内容怎么进入你的订阅。"
          >
            <Select
              ariaLabel="处理方式"
              value={drawer.draft.mode}
              options={[
                { value: 'aggregate', label: '聚合', desc: '播放时读取源，不在本地保存频道。' },
                { value: 'sync', label: '托管', desc: '频道保存到本地，可以分组和授权。' },
              ]}
              onChange={(next) => patch({ mode: next })}
            />
          </Field>
          {drawer.draft.mode === 'sync' && (
            <p className="muted">
              托管源会建立本地分组。删除源或改回聚合时，系统会先问你要不要保留内容。
            </p>
          )}
          <hr className="divider" />
          <h3>分组</h3>
          <div className="form-grid">
            <Field label="分组方式" tip="选择如何整理源里的分组。表格里单独设置了目标的优先。">
              <Select
                ariaLabel="分组方式"
                value={drawer.draft.group_mode}
                options={[
                  { value: 'keep', label: '保留原名', desc: '分组名不变。' },
                  { value: 'map', label: '按映射改名', desc: '按下面的改名表更换分组名。' },
                  { value: 'merge', label: '全部并成一组', desc: '所有分组放进同一个分组。' },
                ]}
                onChange={(next) => patch({ group_mode: next })}
                compact
              />
            </Field>
            {drawer.draft.group_mode === 'merge' && (
              <label>
                合并到分组
                <Tip text="未单独设置目标的分组都放进这个分组。请填写分组名。" />
                <input
                  value={drawer.draft.merge_group}
                  placeholder="目标分组名"
                  onChange={(event) => patch({ merge_group: event.target.value })}
                />
              </label>
            )}
            {showUnlistedGroups && (
              <Field label="映射外的分组" tip="没有写进改名表的分组怎么处理。">
                <Select
                  ariaLabel="映射外的分组"
                  value={drawer.draft.unlisted_groups}
                  options={[
                    { value: 'drop', label: '丢弃', desc: '不导入这些分组。' },
                    { value: 'keep', label: '保留', desc: '按原样或合并规则导入。' },
                  ]}
                  onChange={(next) => patch({ unlisted_groups: next })}
                  compact
                />
              </Field>
            )}
            {drawer.draft.mode === 'sync' && (
              <label>
                分组名前缀
                <Tip text="加到每个分组名的前面。留空时保留源里的原名。" />
                <input
                  value={drawer.draft.group_prefix}
                  placeholder="留空则保留原名"
                  onChange={(event) => patch({ group_prefix: event.target.value })}
                />
              </label>
            )}
          </div>
          <div className="row">
            <button className="ghost" disabled={drawer.busy} onClick={() => void readGroups()}>
              读取分组
            </button>
            <Tip text="只看源里有哪些分组、各有多少频道。不会修改设置。" />
            {dropsAllGroups && (
              <span className="msg warn">
                分组方式是「按映射改名」，但改名表为空，而且映射外的分组设为「丢弃」。这样不会导入任何频道。请添加映射，或把「映射外的分组」改为「保留」。
              </span>
            )}
          </div>
          {drawer.groups && drawer.groups.length > 0 && (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>源里的分组</th>
                    <th className="right">频道</th>
                    <th>目标分组</th>
                    <th className="right">操作</th>
                  </tr>
                </thead>
                <tbody>
                  {drawer.groups.map((group) => {
                    const targetMode = targetModeOf(drawer.draft, group.name);
                    return (
                      <tr key={group.name}>
                        <td>{group.name}</td>
                        <td className="right">{group.count}</td>
                        <td>
                          {drawer.draft.mode === 'sync' ? (
                            <div className="target-picker">
                              <Select
                                ariaLabel={`${group.name} 目标方式`}
                                value={targetMode}
                                options={[
                                  {
                                    value: 'default',
                                    label: '按分组方式',
                                    desc: '使用上面的分组规则。',
                                  },
                                  {
                                    value: 'new',
                                    label: '合并到新组',
                                    desc: '输入新组名；同名会合并。',
                                  },
                                  {
                                    value: 'existing',
                                    label: '并入已有组',
                                    desc: '保持该分组的名称与设置。',
                                  },
                                ]}
                                onChange={(next) =>
                                  setTargetMode(
                                    group.name,
                                    next as 'default' | 'new' | 'existing',
                                  )
                                }
                                compact
                              />
                              {targetMode === 'new' && (
                                <input
                                  aria-label={`${group.name} 新组名`}
                                  value={drawer.draft.merge_new[group.name] ?? ''}
                                  placeholder="新组名"
                                  title="多个上游分组使用同一个新组名时会合并到同一个组。"
                                  onChange={(event) => {
                                    patch({
                                      merge_new: {
                                        ...drawer.draft.merge_new,
                                        [group.name]: event.target.value,
                                      },
                                    });
                                  }}
                                />
                              )}
                              {targetMode === 'existing' && (
                                <Select
                                  ariaLabel={`${group.name} 并入分组`}
                                  value={drawer.draft.merge_into[group.name] ?? ''}
                                  placeholder="选择已有分组"
                                  options={[
                                    { value: '', label: '选择已有分组' },
                                    ...existingGroupOptions,
                                  ]}
                                  onChange={(next) => {
                                    patch({
                                      merge_into: {
                                        ...drawer.draft.merge_into,
                                        [group.name]: next,
                                      },
                                    });
                                  }}
                                  compact
                                />
                              )}
                            </div>
                          ) : (
                            <span className="muted">仅托管源</span>
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
                            加入改名表
                          </button>
                        </td>
                      </tr>
                    );
                  })}
                </tbody>
              </table>
            </div>
          )}
          <label>
            分组改名表
            <Tip text="每行一条「源里的分组=新分组名」。分组方式为「按映射改名」时生效。" />
            <textarea
              value={mapText(drawer.draft.group_map)}
              onChange={(event) => patch({ group_map: parseMap(event.target.value) })}
            />
          </label>
          <div className="form-grid two">
            <label>
              只保留
              <Tip text="只保留匹配的频道。每行一条。支持 *（任意长度）和 ?（一个字符）。开头可写 group: 匹配分组，id: 匹配 tvg-id，url: 精确匹配地址。不含通配符时按子串匹配。" />
              <textarea
                value={drawer.draft.include.join('\n')}
                onChange={(event) => patch({ include: lines(event.target.value) })}
              />
            </label>
            <label>
              排除
              <Tip text="匹配的频道会被排除。支持 * 与 ? 通配符。在列表里删除托管频道时，会自动写入这里。" />
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
              <div className="form-grid">
                <Field
                  label="调度"
                  tip="这个源多久同步一次。"
                  hint={drawer.draft.schedule === 'inherit' ? '使用全局设置' : undefined}
                >
                  <Select
                    ariaLabel="调度"
                    value={drawer.draft.schedule}
                    options={[
                      { value: 'inherit', label: '继承全局', desc: '使用全局设置。' },
                      { value: 'interval', label: '按间隔', desc: '每隔一段时间同步。' },
                      { value: 'daily', label: '每天定时', desc: '每天在固定时间同步。' },
                    ]}
                    onChange={(next) => patch({ schedule: next })}
                    compact
                  />
                </Field>
                {drawer.draft.schedule === 'interval' && (
                  <label>
                    间隔（秒）
                    <NumberBox
                      min={0}
                      value={drawer.draft.sync_interval_secs}
                      onChange={(next) =>
                        patch({ sync_interval_secs: Number(next) || 0 })
                      }
                    />
                    <span className="hint">0 表示使用全局值</span>
                  </label>
                )}
                {drawer.draft.schedule === 'daily' && (
                  <label>
                    每天时间
                    <DateInput
                      type="time"
                      ariaLabel="每天时间"
                      value={drawer.draft.sync_daily_at}
                      onChange={(next) => patch({ sync_daily_at: next })}
                    />
                    <span className="hint">留空表示使用全局值</span>
                  </label>
                )}
              </div>
              <div className="form-grid">
                <Field
                  label="新分组工作模式"
                  tip="只影响之后新建的同步分组。已有分组在「分组管理」里修改「工作模式」。"
                >
                  <Select
                    ariaLabel="新分组工作模式"
                    value={drawer.draft.playback_mode}
                    options={[
                      { value: 'auto', label: '自动选择', desc: '按源的类型自动选。' },
                      { value: 'proxy', label: '服务器转发', desc: '视频经过本机，兼容性最好。' },
                      { value: 'remux', label: '服务器转封装', desc: '本机重新打包视频流。占用 CPU。' },
                      { value: 'dash', label: 'DASH 就绪', desc: '用于 DASH 清单。先确认源可用。' },
                      { value: 'redirect', label: '跳转源地址', desc: '播放器直接连源。会显示源地址。' },
                      { value: 'rewrite', label: '改写地址', desc: '本机改写清单地址，播放器再连源。' },
                    ]}
                    onChange={(next) => patch({ playback_mode: next })}
                    compact
                  />
                </Field>
                <Field
                  label="新分组内容格式"
                  tip="只影响之后新建的同步分组。已有分组沿用分组自己的内容格式。"
                >
                  <Select
                    ariaLabel="新分组内容格式"
                    value={drawer.draft.format}
                    options={[
                      { value: 'auto', label: '自动', desc: '跟随源的类型。' },
                      { value: 'hls', label: 'HLS', desc: 'm3u8 清单。' },
                      { value: 'mpd', label: 'DASH', desc: 'mpd 清单。' },
                    ]}
                    onChange={(next) => patch({ format: next })}
                    compact
                  />
                </Field>
                <Field label="插入位置" tip="这个源的内容放在订阅的哪一端。">
                  <Select
                    ariaLabel="插入位置"
                    value={drawer.draft.position}
                    options={[
                      { value: '', label: '继承全局', desc: '使用全局设置。' },
                      { value: 'top', label: '顶部', desc: '放在订阅开头。' },
                      { value: 'bottom', label: '底部', desc: '放在订阅末尾。' },
                    ]}
                    onChange={(next) => patch({ position: next })}
                    compact
                  />
                </Field>
                <label>
                  单源上限
                  <Tip text="这个源最多导入多少条频道。" />
                  <NumberBox
                    min={0}
                    value={drawer.draft.max_channels}
                    onChange={(next) => patch({ max_channels: Number(next) || 0 })}
                  />
                  <span className="hint">0 表示使用全局值</span>
                </label>
              </div>
              <p className="muted">
                已有分组的工作模式与内容格式在「分组管理」里修改；这里只设置之后新建分组的默认值。
              </p>
            </>
          )}

          <hr className="divider" />
          <h3>高级</h3>
          <label>
            抓取请求头
            <Tip text="每行一条「名称: 值」。读取播放列表时带上。" />
            <textarea
              value={drawer.draft.headers.join('\n')}
              onChange={(event) => patch({ headers: lines(event.target.value) })}
            />
          </label>
          <div className="form-grid two">
            <label>
              代理类型
              <Tip text="读取源时使用的代理协议。一般不用填。" />
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
              <Tip text="代理服务器地址，格式为 host:port。" />
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
          <div className="form-grid two">
            <label>
              代理用户名
              <Tip text="代理需要登录时填写。" />
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
              <Tip text="代理需要登录时填写。" />
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
            <Tip text="先试读一次源，看看规则的效果。不会保存设置。" />
          </div>
          {drawer.preview && (
            <>
              <p className="muted">
                源里共 {drawer.preview.trace.total} 个频道，保留 {drawer.preview.kept} 个。
              </p>
              {drawer.preview.sync && (
                <div className="row">
                  <Badge tone="ok">新增 {drawer.preview.sync.add}</Badge>
                  <Badge tone="info">更新 {drawer.preview.sync.update}</Badge>
                  <Badge tone="warn">移除 {drawer.preview.sync.remove}</Badge>
                  {drawer.preview.sync.groups_add > 0 && (
                    <Badge tone="ok">新增分组 {drawer.preview.sync.groups_add}</Badge>
                  )}
                  {drawer.preview.sync.groups_rename > 0 && (
                    <Badge tone="info">分组改名 {drawer.preview.sync.groups_rename}</Badge>
                  )}
                  {drawer.preview.sync.groups_remove > 0 && (
                    <Badge tone="warn">移除分组 {drawer.preview.sync.groups_remove}</Badge>
                  )}
                  <Badge tone="muted">重复 {drawer.preview.sync.skipped_duplicates}</Badge>
                  {drawer.preview.sync.guard.over_limit && (
                    <Badge tone="danger">超过单源上限</Badge>
                  )}
                  {drawer.preview.sync.guard.empty && (
                    <Badge tone="warn">空清单会阻止清空</Badge>
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
                保留为本地频道
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
              : `「${pending.source.name}」已有 ${pending.channels} 个频道。切换为聚合后，这些频道不再跟随源。`}
          </p>
          <p className="muted">保留为本地频道：频道和分组都保留，只是不再跟随源。</p>
          <p className="muted">一并删除：删除这个源创建的分组和频道。</p>
        </Modal>
      )}

      {node}
    </>
  );
}
