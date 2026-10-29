/** 分组与频道管理。
 *
 * 版式基准：`docs/design/reference/img/p02-groups.png`（列表）、
 * `m01-group-edit.png` 与 `m02-channel-edit.png`（弹窗）。
 *
 * 频道上的 `tvg_id` / `tvg_name` / `logo` 和 `resolver_*` 都存在配置的
 * 未知字段里（`Channel.extra`），后端已经消费它们，所以这里直接读写同名键。
 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { Icon } from './icons';
import { Badge, Confirm, Field, Modal, PageHead, Switch, useToast } from './ui';

export type Channel = {
  id: string;
  name: string;
  url: string;
  /** 本地回看（`常驻` 徽章的来源）。 */
  livestream?: boolean;
  /** DASH 轨道选择；空数组 = 全轨道（`全轨道` 徽章的来源）。 */
  selected_tracks?: string[];
  [key: string]: unknown;
};

export type Group = {
  id: string;
  name: string;
  mode: string;
  access: string;
  format: string;
  channels?: Channel[];
  [key: string]: unknown;
};

/** `/panel/api/health` 的单频道条目。 */
export type HealthEntry = {
  checked_at: number;
  consecutive_fails: number;
  error: string | null;
  http_status: number | null;
  latency_ms: number | null;
  ok: boolean;
};

const MODES: { value: string; label: string }[] = [
  { value: 'proxy', label: 'Proxy（反向代理）' },
  { value: 'remux', label: 'Remux（重封装）' },
  { value: 'dash', label: 'DASH（MPD 转 HLS）' },
  { value: 'redirect', label: 'Redirect（302 跳转）' },
  { value: 'rewrite', label: 'Rewrite（改写清单）' },
  { value: 'passthrough', label: 'Passthrough（直通）' },
];

const ACCESS: { value: string; label: string }[] = [
  { value: 'public', label: '公开' },
  { value: 'hidden', label: '隐藏' },
  { value: 'open', label: '开放（免 Token）' },
  { value: 'archived', label: '归档' },
];

const FORMATS: { value: string; label: string }[] = [
  { value: 'hls', label: 'HLS（m3u8）' },
  { value: 'mpd', label: 'DASH（mpd）' },
];

/** 播放地址后缀由分组格式决定（S-URL-03）。 */
function entryExt(group: Group): string {
  return group.format === 'mpd' ? 'mpd' : 'm3u8';
}

function entryUrl(group: Group, channel: Channel): string {
  return `${location.origin}/${group.id}/${channel.id}.${entryExt(group)}`;
}

/** 读频道配置里的字符串字段；空串按未设置处理。 */
function text(channel: Channel, key: string): string {
  const value = channel[key];
  return typeof value === 'string' ? value : '';
}

type ProxyForm = { kind: string; addr: string; user: string; pass: string };
type GroupForm = {
  id: string;
  name: string;
  mode: string;
  access: string;
  format: string;
  headers: string;
  proxy: ProxyForm;
};
type ChannelForm = {
  id: string;
  name: string;
  url: string;
  logo: string;
  tvgId: string;
  tvgName: string;
  /** `static` = 直接用 URL；`script` = 交给解析脚本。 */
  upstream: 'static' | 'script';
  script: string;
  scriptArg: string;
  scriptTtl: string;
  livestream: boolean;
  timeshiftHours: string;
  timeshiftMaxGb: string;
  headers: string;
  proxy: ProxyForm;
};

/**
 * 预取覆盖的三态（`PrefetchOverrideCfg` 全是 `Option`）。
 *
 * - `''` 跟随全局：整个覆盖清空；
 * - `'inherit'` 只覆盖数量/并发，开关仍取全局值；
 * - `'on'` / `'off'` 显式开或关。
 */
type PrefetchForm = {
  state: '' | 'inherit' | 'on' | 'off';
  limit: string;
  concurrency: string;
};

const PREFETCH_OPTIONS: { value: PrefetchForm['state']; label: string }[] = [
  { value: '', label: '跟随全局' },
  { value: 'inherit', label: '只覆盖数量与并发' },
  { value: 'on', label: '开启预取' },
  { value: 'off', label: '关闭预取' },
];

function emptyProxy(): ProxyForm {
  return { kind: 'http', addr: '', user: '', pass: '' };
}

function emptyPrefetch(): PrefetchForm {
  return { state: '', limit: '', concurrency: '' };
}

function proxyForm(raw: unknown): ProxyForm {
  if (!raw || typeof raw !== 'object') return emptyProxy();
  const value = raw as { kind?: string; addr?: string; username?: string; password?: string };
  return {
    kind: value.kind || 'http',
    addr: value.addr ?? '',
    user: value.username ?? '',
    pass: value.password ?? '',
  };
}

/** 表单 → `proxy` 字段；地址为空表示清空。 */
function proxyPayload(form: ProxyForm): Record<string, unknown> | null {
  if (form.addr.trim() === '') return null;
  return {
    kind: form.kind || 'http',
    addr: form.addr.trim(),
    username: form.user.trim() === '' ? undefined : form.user,
    password: form.pass.trim() === '' ? undefined : form.pass,
  };
}

/** 表单 → 后端 `prefetch` 字段；`null` 表示清空覆盖。 */
function prefetchPayload(form: PrefetchForm): Record<string, unknown> | null {
  if (form.state === '') return null;
  const payload: Record<string, unknown> = {};
  if (form.state !== 'inherit') payload.enabled = form.state === 'on';
  const limit = form.limit.trim();
  if (limit !== '') payload.prefetch_limit = Number(limit);
  const concurrency = form.concurrency.trim();
  if (concurrency !== '') payload.concurrency = Number(concurrency);
  return payload;
}

/** 后端 `prefetch` 字段 → 表单。 */
function prefetchForm(raw: unknown): PrefetchForm {
  if (!raw || typeof raw !== 'object') return emptyPrefetch();
  const value = raw as { enabled?: boolean; prefetch_limit?: number; concurrency?: number };
  const state: PrefetchForm['state'] =
    value.enabled === true ? 'on' : value.enabled === false ? 'off' : 'inherit';
  return {
    state,
    limit: value.prefetch_limit === undefined ? '' : String(value.prefetch_limit),
    concurrency: value.concurrency === undefined ? '' : String(value.concurrency),
  };
}

/** 列表里的预取标记；没有覆盖时不显示。 */
function PrefetchBadge({ raw }: { raw: unknown }) {
  if (!raw || typeof raw !== 'object') return null;
  const form = prefetchForm(raw);
  const label =
    form.state === 'off'
      ? '预取关闭'
      : form.state === 'on'
        ? '预取开启'
        : `预取 ${form.limit || '—'} 片 / ${form.concurrency || '—'} 并发`;
  return <Badge tone={form.state === 'off' ? 'warn' : 'muted'}>{label}</Badge>;
}

/** 网络配置区：代理 + 请求头 + 预取覆盖。三处共用。 */
function NetworkBlock({
  proxy,
  onProxy,
  headers,
  onHeaders,
  prefetch,
  onPrefetch,
  summary,
}: {
  proxy: ProxyForm;
  onProxy: (next: ProxyForm) => void;
  headers: string;
  onHeaders: (next: string) => void;
  prefetch: PrefetchForm;
  onPrefetch: (next: PrefetchForm) => void;
  summary: string;
}) {
  const numeric = prefetch.state !== '';
  return (
    <details className="block">
      <summary>
        <Icon name="settings" size={13} />
        {summary}
      </summary>
      <div className="fields">
        <Field label="代理类型">
          <select value={proxy.kind} onChange={(e) => onProxy({ ...proxy, kind: e.target.value })}>
            <option value="http">http</option>
            <option value="https">https</option>
            <option value="socks5">socks5</option>
          </select>
        </Field>
        <Field label="代理地址">
          <input
            placeholder="127.0.0.1:8080，留空表示不用"
            value={proxy.addr}
            onChange={(e) => onProxy({ ...proxy, addr: e.target.value })}
          />
        </Field>
        <Field label="代理用户名">
          <input value={proxy.user} onChange={(e) => onProxy({ ...proxy, user: e.target.value })} />
        </Field>
        <Field label="代理密码">
          <input
            type="password"
            value={proxy.pass}
            onChange={(e) => onProxy({ ...proxy, pass: e.target.value })}
          />
        </Field>
      </div>
      <Field label="请求头" hint="每行一条，格式 `名称: 值`。留空表示不加。">
        <textarea
          rows={3}
          placeholder="User-Agent: VLC/3.0"
          value={headers}
          onChange={(e) => onHeaders(e.target.value)}
        />
      </Field>
      <Field label="预取覆盖" hint="留空继承上一级；数值留空表示不覆盖。">
        <select
          value={prefetch.state}
          onChange={(e) => onPrefetch({ ...prefetch, state: e.target.value as PrefetchForm['state'] })}
        >
          {PREFETCH_OPTIONS.map((option) => (
            <option key={option.value} value={option.value}>
              {option.label}
            </option>
          ))}
        </select>
      </Field>
      {numeric && (
        <div className="fields">
          <Field label="单次上限（0 = 不限）">
            <input
              type="number"
              min={0}
              value={prefetch.limit}
              placeholder="留空不覆盖"
              onChange={(e) => onPrefetch({ ...prefetch, limit: e.target.value })}
            />
          </Field>
          <Field label="并发数（1-32）">
            <input
              type="number"
              min={1}
              max={32}
              value={prefetch.concurrency}
              placeholder="留空不覆盖"
              onChange={(e) => onPrefetch({ ...prefetch, concurrency: e.target.value })}
            />
          </Field>
        </div>
      )}
    </details>
  );
}

/** 请求头数组 ↔ 多行文本。 */
function headersToText(raw: unknown): string {
  if (!Array.isArray(raw)) return '';
  return raw.filter((item): item is string => typeof item === 'string').join('\n');
}

function textToHeaders(value: string): string[] {
  return value
    .split('\n')
    .map((line) => line.trim())
    .filter((line) => line !== '');
}

export function GroupsPage() {
  const { node, ok, err } = useToast();
  const [groups, setGroups] = useState<Group[]>([]);
  const [filter, setFilter] = useState('');
  const [groupForm, setGroupForm] = useState<{
    form: GroupForm;
    prefetch: PrefetchForm;
    isNew: boolean;
  } | null>(null);
  const [channelForm, setChannelForm] = useState<{
    gid: string;
    form: ChannelForm;
    prefetch: PrefetchForm;
    isNew: boolean;
  } | null>(null);
  const [removing, setRemoving] = useState<
    { kind: 'group' | 'channel'; gid: string; cid?: string; label: string } | null
  >(null);
  const [importing, setImporting] = useState<{ kind: 'json' | 'm3u'; force: boolean } | null>(null);
  const [importText, setImportText] = useState('');
  const [m3uMode, setM3uMode] = useState<'append' | 'merge' | 'replace'>('append');
  const [health, setHealth] = useState<Record<string, HealthEntry>>({});
  const [tracks, setTracks] = useState<{ gid: string; cid: string; body: string } | null>(null);
  /** 多选：批量删除与批量导出。 */
  const [multi, setMulti] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  /** 解析脚本列表；打开频道弹窗时按需加载。 */
  const [scripts, setScripts] = useState<string[]>([]);

  const load = useCallback(async () => {
    try {
      setGroups(await api<Group[]>('GET', '/groups/export'));
      // 逐频道探测结果；未启用探测时为空对象。
      const body = await api<{ entries?: Record<string, HealthEntry> }>('GET', '/health');
      setHealth(body.entries ?? {});
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const loadScripts = useCallback(async () => {
    try {
      const body = await api<{ scripts?: { name: string }[] }>('GET', '/resolver/scripts');
      setScripts((body.scripts ?? []).map((item) => item.name));
    } catch {
      // 没有脚本目录时保持空列表。
    }
  }, []);

  const saveGroup = async () => {
    if (!groupForm) return;
    const { form, prefetch, isNew } = groupForm;
    const payload = {
      name: form.name,
      mode: form.mode,
      access: form.access,
      format: form.format,
      headers: textToHeaders(form.headers),
      proxy: proxyPayload(form.proxy),
      prefetch: prefetchPayload(prefetch),
    };
    try {
      if (isNew) {
        await api('POST', '/groups', { id: form.id, ...payload });
      } else {
        await api('PUT', `/groups/${form.id}`, payload);
      }
      setGroupForm(null);
      ok(isNew ? '分组已创建' : '分组已保存');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const saveChannel = async () => {
    if (!channelForm) return;
    const { gid, form, prefetch, isNew } = channelForm;
    // `Channel.extra` 不能存 null，清空字段一律写空串。
    const payload: Record<string, unknown> = {
      name: form.name,
      url: form.url,
      logo: form.logo.trim(),
      tvg_id: form.tvgId.trim(),
      tvg_name: form.tvgName.trim(),
      resolver_script: form.upstream === 'script' ? form.script.trim() : '',
      resolver_arg: form.upstream === 'script' ? form.scriptArg : '',
      resolver_ttl: form.upstream === 'script' ? Number(form.scriptTtl) || 30 : 30,
      livestream: form.livestream,
      timeshift_hours: form.livestream ? Number(form.timeshiftHours) || 0 : 0,
      timeshift_max_gb: form.livestream ? Number(form.timeshiftMaxGb) || 0 : 0,
      headers: textToHeaders(form.headers),
      proxy: proxyPayload(form.proxy),
      prefetch: prefetchPayload(prefetch),
    };
    try {
      if (isNew) {
        await api('POST', `/groups/${gid}/channels`, { id: form.id, ...payload });
      } else {
        await api('PUT', `/groups/${gid}/channels/${form.id}`, payload);
      }
      setChannelForm(null);
      ok(isNew ? '频道已创建' : '频道已保存');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const doRemove = async () => {
    if (!removing) return;
    const { kind, gid, cid } = removing;
    try {
      if (kind === 'group') {
        await api('DELETE', `/groups/${gid}`);
        ok('分组已删除');
      } else {
        await api('DELETE', `/groups/${gid}/channels/${cid}`);
        ok('频道已删除');
      }
      setRemoving(null);
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const copy = async (value: string, label: string) => {
    const done = await copyText(value);
    if (done) ok(`${label}已复制`);
    else err('复制失败，请手动选取');
  };

  /**
   * 重排后整份回写。
   *
   * 目标没有单独的排序端点：它在前端改数组顺序后 `PUT /panel/api/config` 全量保存，
   * 本实现与之一致。播放列表按 `[[groups]]` / `[[groups.channels]]` 的数组顺序输出
   * （`playlist::render_total`），所以顺序即订阅里的顺序。
   */
  const reorder = async (mutate: (list: Group[]) => boolean) => {
    try {
      const config = await api<{ groups?: Group[] } & Record<string, unknown>>('GET', '/config');
      const list = config.groups ?? [];
      if (!mutate(list)) return;
      await api('PUT', '/config', { ...config, groups: list });
      ok('顺序已保存');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const moveGroup = (index: number, delta: number) =>
    reorder((list) => {
      const target = index + delta;
      if (target < 0 || target >= list.length) return false;
      const [moved] = list.splice(index, 1);
      list.splice(target, 0, moved);
      return true;
    });

  const moveChannel = (gid: string, index: number, delta: number) =>
    reorder((list) => {
      const channels = list.find((item) => item.id === gid)?.channels;
      if (!channels) return false;
      const target = index + delta;
      if (target < 0 || target >= channels.length) return false;
      const [moved] = channels.splice(index, 1);
      channels.splice(target, 0, moved);
      return true;
    });

  const submitImport = async () => {
    if (!importing) return;
    const { kind, force } = importing;
    try {
      if (kind === 'm3u') {
        const result = await api<{
          parsed_channels: number;
          added_groups: number;
          merged_channels: number;
          replaced_groups: number;
          skipped_groups: number;
        }>('POST', '/groups/import/m3u', { text: importText, mode: m3uMode });
        ok(
          `已解析 ${result.parsed_channels} 个频道：新增 ${result.added_groups} 组、合并 ${result.merged_channels} 个频道、替换 ${result.replaced_groups} 组、跳过 ${result.skipped_groups} 组`,
        );
      } else {
        let parsed: unknown;
        try {
          parsed = JSON.parse(importText);
        } catch {
          err('JSON 解析失败，请检查格式');
          return;
        }
        if (!Array.isArray(parsed)) {
          err('JSON 必须是分组数组');
          return;
        }
        const result = await api<{ imported: number; skipped: number }>(
          'POST',
          force ? '/groups/import/force' : '/groups/import',
          parsed,
        );
        ok(
          force
            ? `已覆盖导入 ${result.imported} 个分组`
            : `已导入 ${result.imported} 个分组，跳过 ${result.skipped} 个同 ID`,
        );
      }
      setImporting(null);
      setImportText('');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  /** 导出全部：把 `GET /groups/export` 的原文下载成 `groups.json`。 */
  const exportAll = async () => {
    await download('/groups/export', 'groups.json', '分组已导出');
  };

  /** 导出单个分组：`GET /groups/{gid}/export`。 */
  const exportGroup = async (gid: string) => {
    await download(`/groups/${gid}/export`, `${gid}.json`, `分组 ${gid} 已导出`);
  };

  const download = async (path: string, filename: string, done: string) => {
    try {
      const data = await api<unknown>('GET', path);
      const blob = new Blob([JSON.stringify(data, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = filename;
      anchor.click();
      URL.revokeObjectURL(url);
      ok(done);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  /** DASH 轨道预览：`GET /dash/tracks/{gid}/{cid}`。 */
  const showTracks = async (gid: string, cid: string) => {
    try {
      const data = await api<{ tracks?: unknown[] }>('GET', `/dash/tracks/${gid}/${cid}`);
      setTracks({ gid, cid, body: JSON.stringify(data.tracks ?? data, null, 2) });
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const toggleSelected = (gid: string) => {
    setSelected((current) =>
      current.includes(gid) ? current.filter((item) => item !== gid) : [...current, gid],
    );
  };

  /** 批量导出：逐个 `GET /groups/{gid}/export`，合成一个 JSON 数组。 */
  const exportSelected = async () => {
    try {
      const items = await Promise.all(
        selected.map((gid) => api<unknown>('GET', `/groups/${gid}/export`)),
      );
      const blob = new Blob([JSON.stringify(items, null, 2)], { type: 'application/json' });
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = 'groups-selected.json';
      anchor.click();
      URL.revokeObjectURL(url);
      ok(`已导出 ${items.length} 个分组`);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  /** 批量删除：逐个 `DELETE /groups/{gid}`。 */
  const removeSelected = async () => {
    if (!window.confirm(`确认删除选中的 ${selected.length} 个分组？此操作不可撤销。`)) return;
    try {
      for (const gid of selected) {
        await api('DELETE', `/groups/${gid}`);
      }
      ok(`已删除 ${selected.length} 个分组`);
      setSelected([]);
      setMulti(false);
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const openGroup = (group?: Group) => {
    setGroupForm({
      form: group
        ? {
            id: group.id,
            name: group.name,
            mode: group.mode,
            access: group.access,
            format: group.format,
            headers: headersToText(group.headers),
            proxy: proxyForm(group.proxy),
          }
        : {
            id: '',
            name: '',
            mode: 'proxy',
            access: 'public',
            format: 'hls',
            headers: '',
            proxy: emptyProxy(),
          },
      prefetch: prefetchForm(group?.prefetch),
      isNew: !group,
    });
  };

  const openChannel = (gid: string, channel?: Channel) => {
    void loadScripts();
    const script = channel ? text(channel, 'resolver_script') : '';
    setChannelForm({
      gid,
      form: channel
        ? {
            id: channel.id,
            name: channel.name,
            url: channel.url,
            logo: text(channel, 'logo') || text(channel, 'tvg_logo'),
            tvgId: text(channel, 'tvg_id'),
            tvgName: text(channel, 'tvg_name'),
            upstream: script === '' ? 'static' : 'script',
            script,
            scriptArg: text(channel, 'resolver_arg'),
            scriptTtl: String(channel.resolver_ttl ?? 30),
            livestream: channel.livestream === true,
            timeshiftHours: String(channel.timeshift_hours ?? 4),
            timeshiftMaxGb: String(channel.timeshift_max_gb ?? 0),
            headers: headersToText(channel.headers),
            proxy: proxyForm(channel.proxy),
          }
        : {
            id: '',
            name: '',
            url: '',
            logo: '',
            tvgId: '',
            tvgName: '',
            upstream: 'static',
            script: '',
            scriptArg: '',
            scriptTtl: '30',
            livestream: false,
            timeshiftHours: '4',
            timeshiftMaxGb: '0',
            headers: '',
            proxy: emptyProxy(),
          },
      prefetch: prefetchForm(channel?.prefetch),
      isNew: !channel,
    });
  };

  const keyword = filter.trim().toLowerCase();
  const visible = keyword
    ? groups.filter((group) => {
        if (`${group.id} ${group.name}`.toLowerCase().includes(keyword)) return true;
        return (group.channels ?? []).some((channel) =>
          `${channel.id} ${channel.name}`.toLowerCase().includes(keyword),
        );
      })
    : groups;

  return (
    <>
      <PageHead icon="groups" title="分组管理" sub="管理直播分组与频道">
        <input
          className="search"
          placeholder="搜索分组或频道"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
        <button
          className="ghost"
          onClick={() => {
            setImporting({ kind: 'm3u', force: false });
            setImportText('');
          }}
        >
          <Icon name="upload" size={14} />
          M3U 导入
        </button>
        <button
          className="ghost"
          onClick={() => {
            setImporting({ kind: 'json', force: false });
            setImportText('');
          }}
        >
          <Icon name="folder" size={14} />
          JSON 导入
        </button>
        <button
          className="ghost"
          onClick={() => {
            setImporting({ kind: 'json', force: true });
            setImportText('');
          }}
        >
          <Icon name="folder" size={14} />
          JSON 覆盖
        </button>
        <button className="ghost" onClick={() => void exportAll()}>
          <Icon name="download" size={14} />
          导出全部
        </button>
        <button
          className={multi ? '' : 'ghost'}
          onClick={() => {
            setMulti(!multi);
            setSelected([]);
          }}
        >
          <Icon name="check" size={14} />
          多选
        </button>
        <button onClick={() => openGroup()}>
          <Icon name="plus" size={14} />
          新建分组
        </button>
      </PageHead>

      {multi && selected.length > 0 && (
        <div className="card">
          <div className="row between">
            <span>已选 {selected.length} 个分组</span>
            <div className="row">
              <button className="ghost" onClick={() => void exportSelected()}>
                批量导出
              </button>
              <button className="ghost danger-text" onClick={() => void removeSelected()}>
                批量删除
              </button>
            </div>
          </div>
        </div>
      )}

      {visible.length === 0 && (
        <div className="card muted">{groups.length === 0 ? '还没有分组。' : '没有匹配的分组。'}</div>
      )}

      {visible.map((group) => {
        const channels = group.channels ?? [];
        const listUrl = `${location.origin}/${group.id}/playlist.m3u`;
        // 排序按 `groups` 的绝对下标；搜索过滤时也不会错位。
        const groupIndex = groups.indexOf(group);
        return (
          <div className="card flush group-card" key={group.id}>
            <div className="group-head">
              {multi && (
                <input
                  type="checkbox"
                  style={{ width: 'auto', height: 'auto' }}
                  title="选择分组"
                  checked={selected.includes(group.id)}
                  onChange={() => toggleSelected(group.id)}
                />
              )}
              <Icon name="folder" size={15} className="folder" />
              <span className="gname">{group.name || group.id}</span>
              <span className="gid">[{group.id}]</span>
              <div className="ops">
                <span className={`badge mode ${group.mode}`}>{group.mode}</span>
                <span className="count">{channels.length} 频道</span>
                <button
                  className="icon-btn move"
                  disabled={groupIndex <= 0}
                  title="上移分组"
                  onClick={() => void moveGroup(groupIndex, -1)}
                >
                  <Icon name="up" size={12} />
                </button>
                <button
                  className="icon-btn move"
                  disabled={groupIndex < 0 || groupIndex >= groups.length - 1}
                  title="下移分组"
                  onClick={() => void moveGroup(groupIndex, 1)}
                >
                  <Icon name="down" size={12} />
                </button>
                <button className="chip" onClick={() => openGroup(group)}>
                  编辑
                </button>
                <button
                  className="chip icon-only"
                  title="导出该分组的 JSON"
                  onClick={() => void exportGroup(group.id)}
                >
                  <Icon name="download" size={13} />
                </button>
                <button
                  className="chip danger"
                  onClick={() =>
                    setRemoving({ kind: 'group', gid: group.id, label: group.name || group.id })
                  }
                >
                  删除
                </button>
                <button className="chip primary" onClick={() => openChannel(group.id)}>
                  <Icon name="plus" size={13} />
                  频道
                </button>
                <a className="chip" href={listUrl} target="_blank" rel="noreferrer" title="打开该分组的 M3U">
                  <Icon name="download" size={13} />
                  M3U
                </a>
                <button
                  className="chip icon-only"
                  title="复制订阅地址"
                  onClick={() => void copy(listUrl, '订阅地址')}
                >
                  <Icon name="link" size={13} />
                </button>
              </div>
            </div>

            {channels.map((channel, channelIndex) => {
              const state = health[`${group.id}/${channel.id}`];
              const logo = text(channel, 'logo') || text(channel, 'tvg_logo');
              const tvgId = text(channel, 'tvg_id');
              const allTracks = group.mode === 'dash' && (channel.selected_tracks ?? []).length === 0;
              const local =
                group.mode === 'passthrough' ? channel.url : entryUrl(group, channel);
              return (
                <div className="ch-row" key={channel.id}>
                  <span
                    className={`dot${state?.ok === true ? ' ok' : state?.ok === false ? ' err' : ''}`}
                    title={state ? (state.ok ? '最近探测成功' : '最近探测失败') : '未启用主动探测'}
                  />
                  <div className="ch-main">
                    <div className="ch-name">
                      <span>{channel.name || channel.id}</span>
                      <span className="cid">[{channel.id}]</span>
                      {state?.ok === false && <Badge tone="warn">异常</Badge>}
                      {channel.livestream === true && <Badge tone="info">常驻</Badge>}
                      {allTracks && <Badge tone="muted">全轨道</Badge>}
                    </div>
                    <div className="ch-url">{channel.url}</div>
                    <div className="ch-url local">
                      {local}
                      {group.mode === 'passthrough' && <Badge tone="muted">直通</Badge>}
                    </div>
                    <div className="ch-badges">
                      {tvgId !== '' && <Badge tone="muted">tvg-id</Badge>}
                      {logo !== '' && <Badge tone="muted">Logo</Badge>}
                      <PrefetchBadge raw={channel.prefetch} />
                    </div>
                  </div>
                  <div className="ch-ops">
                    <button
                      className="icon-btn move"
                      disabled={channelIndex === 0}
                      title="上移频道"
                      onClick={() => void moveChannel(group.id, channelIndex, -1)}
                    >
                      <Icon name="up" size={12} />
                    </button>
                    <button
                      className="icon-btn move"
                      disabled={channelIndex >= channels.length - 1}
                      title="下移频道"
                      onClick={() => void moveChannel(group.id, channelIndex, 1)}
                    >
                      <Icon name="down" size={12} />
                    </button>
                    <button
                      className="chip icon-only"
                      title="复制播放地址"
                      onClick={() => void copy(local, '播放地址')}
                    >
                      <Icon name="copy" size={13} />
                    </button>
                    {group.mode === 'dash' && (
                      <button
                        className="chip icon-only"
                        title="查看轨道"
                        onClick={() => void showTracks(group.id, channel.id)}
                      >
                        <Icon name="search" size={13} />
                      </button>
                    )}
                    <button className="chip" onClick={() => openChannel(group.id, channel)}>
                      编辑
                    </button>
                    <button
                      className="chip danger"
                      onClick={() =>
                        setRemoving({
                          kind: 'channel',
                          gid: group.id,
                          cid: channel.id,
                          label: channel.name || channel.id,
                        })
                      }
                    >
                      删除
                    </button>
                  </div>
                </div>
              );
            })}

            {channels.length === 0 && <div className="ch-empty">这个分组还没有频道。</div>}
          </div>
        );
      })}

      {groupForm && (
        <Modal
          title={groupForm.isNew ? '新建分组' : '编辑分组'}
          onClose={() => setGroupForm(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setGroupForm(null)}>
                取消
              </button>
              <button onClick={() => void saveGroup()}>保存</button>
            </>
          }
        >
          <Field label="分组 ID" hint="只用于地址与配置键，建成后不能改。">
            <input
              value={groupForm.form.id}
              disabled={!groupForm.isNew}
              placeholder="如 cctv"
              onChange={(event) =>
                setGroupForm({ ...groupForm, form: { ...groupForm.form, id: event.target.value } })
              }
            />
          </Field>
          <Field label="显示名称">
            <input
              value={groupForm.form.name}
              placeholder="如 央视频道"
              onChange={(event) =>
                setGroupForm({ ...groupForm, form: { ...groupForm.form, name: event.target.value } })
              }
            />
          </Field>
          <div className="fields">
            <Field label="工作模式">
              <select
                value={groupForm.form.mode}
                onChange={(event) =>
                  setGroupForm({
                    ...groupForm,
                    form: { ...groupForm.form, mode: event.target.value },
                  })
                }
              >
                {MODES.map((mode) => (
                  <option key={mode.value} value={mode.value}>
                    {mode.label}
                  </option>
                ))}
              </select>
            </Field>
            <Field label="内容格式">
              <select
                value={groupForm.form.format}
                onChange={(event) =>
                  setGroupForm({
                    ...groupForm,
                    form: { ...groupForm.form, format: event.target.value },
                  })
                }
              >
                {FORMATS.map((format) => (
                  <option key={format.value} value={format.value}>
                    {format.label}
                  </option>
                ))}
              </select>
            </Field>
          </div>
          <Field label="访问范围">
            <select
              value={groupForm.form.access}
              onChange={(event) =>
                setGroupForm({
                  ...groupForm,
                  form: { ...groupForm.form, access: event.target.value },
                })
              }
            >
              {ACCESS.map((access) => (
                <option key={access.value} value={access.value}>
                  {access.label}
                </option>
              ))}
            </select>
          </Field>
          <NetworkBlock
            summary="网络配置 · 分组级默认，频道可单独覆盖"
            proxy={groupForm.form.proxy}
            onProxy={(proxy) =>
              setGroupForm({ ...groupForm, form: { ...groupForm.form, proxy } })
            }
            headers={groupForm.form.headers}
            onHeaders={(headers) =>
              setGroupForm({ ...groupForm, form: { ...groupForm.form, headers } })
            }
            prefetch={groupForm.prefetch}
            onPrefetch={(prefetch) => setGroupForm({ ...groupForm, prefetch })}
          />
        </Modal>
      )}

      {channelForm && (
        <Modal
          title={channelForm.isNew ? '新建频道' : '编辑频道'}
          onClose={() => setChannelForm(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setChannelForm(null)}>
                取消
              </button>
              <button onClick={() => void saveChannel()}>保存</button>
            </>
          }
        >
          <div className="fields">
            <Field label="频道 ID" hint="建成后不能改。">
              <input
                value={channelForm.form.id}
                disabled={!channelForm.isNew}
                placeholder="如 c1"
                onChange={(event) =>
                  setChannelForm({
                    ...channelForm,
                    form: { ...channelForm.form, id: event.target.value },
                  })
                }
              />
            </Field>
            <Field label="频道名称">
              <input
                value={channelForm.form.name}
                placeholder="如 CCTV-1"
                onChange={(event) =>
                  setChannelForm({
                    ...channelForm,
                    form: { ...channelForm.form, name: event.target.value },
                  })
                }
              />
            </Field>
          </div>
          <div className="fields">
            <Field label="上游 URL">
              <input
                value={channelForm.form.url}
                placeholder="http://upstream/index.m3u8"
                onChange={(event) =>
                  setChannelForm({
                    ...channelForm,
                    form: { ...channelForm.form, url: event.target.value },
                  })
                }
              />
            </Field>
            <Field label="频道 Logo URL">
              <input
                value={channelForm.form.logo}
                placeholder="/logo/x.png 或 https://…"
                onChange={(event) =>
                  setChannelForm({
                    ...channelForm,
                    form: { ...channelForm.form, logo: event.target.value },
                  })
                }
              />
            </Field>
          </div>
          <div className="fields">
            <Field label="tvg-id" hint="留空则同频道 ID。">
              <input
                value={channelForm.form.tvgId}
                onChange={(event) =>
                  setChannelForm({
                    ...channelForm,
                    form: { ...channelForm.form, tvgId: event.target.value },
                  })
                }
              />
            </Field>
            <Field label="tvg-name" hint="留空则同频道名称。">
              <input
                value={channelForm.form.tvgName}
                onChange={(event) =>
                  setChannelForm({
                    ...channelForm,
                    form: { ...channelForm.form, tvgName: event.target.value },
                  })
                }
              />
            </Field>
          </div>

          <hr className="divider" />
          <div className="section-title">上游解析</div>
          <Field label="上游类型">
            <select
              value={channelForm.form.upstream}
              onChange={(event) =>
                setChannelForm({
                  ...channelForm,
                  form: {
                    ...channelForm.form,
                    upstream: event.target.value as 'static' | 'script',
                  },
                })
              }
            >
              <option value="static">静态 URL</option>
              <option value="script">解析脚本</option>
            </select>
          </Field>
          {channelForm.form.upstream === 'script' && (
            <div className="fields">
              <Field label="脚本" hint="位于 <缓存目录>/resolvers/。">
                <select
                  value={channelForm.form.script}
                  onChange={(event) =>
                    setChannelForm({
                      ...channelForm,
                      form: { ...channelForm.form, script: event.target.value },
                    })
                  }
                >
                  <option value="">（请选择）</option>
                  {scripts.map((name) => (
                    <option key={name} value={name}>
                      {name}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="脚本参数">
                <input
                  value={channelForm.form.scriptArg}
                  onChange={(event) =>
                    setChannelForm({
                      ...channelForm,
                      form: { ...channelForm.form, scriptArg: event.target.value },
                    })
                  }
                />
              </Field>
              <Field label="缓存秒数">
                <input
                  type="number"
                  min={0}
                  value={channelForm.form.scriptTtl}
                  onChange={(event) =>
                    setChannelForm({
                      ...channelForm,
                      form: { ...channelForm.form, scriptTtl: event.target.value },
                    })
                  }
                />
              </Field>
            </div>
          )}
          <div className="preview">
            {channelForm.form.upstream === 'script'
              ? `python3 resolvers/${channelForm.form.script || '…'} ${channelForm.form.scriptArg}`.trim()
              : channelForm.form.url || '（尚未填写）'}
            {channelForm.form.upstream === 'static' && channelForm.form.url !== '' && (
              <button
                className="icon-btn"
                title="复制"
                onClick={() => void copy(channelForm.form.url, '上游地址')}
              >
                <Icon name="copy" size={13} />
              </button>
            )}
          </div>

          <details className="block">
            <summary>
              <Icon name="livestream" size={13} />
              回看配置
            </summary>
            <div className="row between" style={{ marginBottom: 8 }}>
              <span className="muted">保留历史切片，支持 ?playseek= 回看。</span>
              <Switch
                checked={channelForm.form.livestream}
                onChange={(livestream) =>
                  setChannelForm({ ...channelForm, form: { ...channelForm.form, livestream } })
                }
              />
            </div>
            {channelForm.form.livestream && (
              <div className="fields">
                <Field label="保留小时数" hint="0 表示不限制。">
                  <input
                    type="number"
                    min={0}
                    value={channelForm.form.timeshiftHours}
                    onChange={(event) =>
                      setChannelForm({
                        ...channelForm,
                        form: { ...channelForm.form, timeshiftHours: event.target.value },
                      })
                    }
                  />
                </Field>
                <Field label="磁盘上限（GB）" hint="0 表示不限制。">
                  <input
                    type="number"
                    min={0}
                    value={channelForm.form.timeshiftMaxGb}
                    onChange={(event) =>
                      setChannelForm({
                        ...channelForm,
                        form: { ...channelForm.form, timeshiftMaxGb: event.target.value },
                      })
                    }
                  />
                </Field>
              </div>
            )}
          </details>

          <NetworkBlock
            summary="网络配置 · 留空则继承分组配置"
            proxy={channelForm.form.proxy}
            onProxy={(proxy) =>
              setChannelForm({ ...channelForm, form: { ...channelForm.form, proxy } })
            }
            headers={channelForm.form.headers}
            onHeaders={(headers) =>
              setChannelForm({ ...channelForm, form: { ...channelForm.form, headers } })
            }
            prefetch={channelForm.prefetch}
            onPrefetch={(prefetch) => setChannelForm({ ...channelForm, prefetch })}
          />
        </Modal>
      )}

      {importing && (
        <Modal
          title={
            importing.kind === 'm3u' ? 'M3U 导入' : importing.force ? 'JSON 覆盖导入' : 'JSON 导入'
          }
          onClose={() => setImporting(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setImporting(null)}>
                取消
              </button>
              <button disabled={importText.trim() === ''} onClick={() => void submitImport()}>
                导入
              </button>
            </>
          }
        >
          {importing.kind === 'm3u' ? (
            <Field label="同名分组处理">
              <select
                value={m3uMode}
                onChange={(event) =>
                  setM3uMode(event.target.value as 'append' | 'merge' | 'replace')
                }
              >
                <option value="append">append —— 同名分组跳过</option>
                <option value="merge">merge —— 同名分组补频道</option>
                <option value="replace">replace —— 同名分组整体替换</option>
              </select>
            </Field>
          ) : (
            <p className="muted">
              {importing.force
                ? '按分组 ID 覆盖：已存在的分组整体替换，其余追加。'
                : '按分组 ID 追加：已存在的分组跳过，其余新增。'}
            </p>
          )}
          <Field label={importing.kind === 'm3u' ? 'M3U 文本' : '分组 JSON 数组'}>
            <textarea
              style={{ minHeight: 300 }}
              placeholder={
                importing.kind === 'm3u'
                  ? '#EXTM3U\n#EXTINF:-1 tvg-id="cctv1" group-title="央视",CCTV-1\nhttp://…'
                  : '[{ "id": "cctv", "name": "央视", "mode": "proxy", "channels": [] }]'
              }
              value={importText}
              onChange={(event) => setImportText(event.target.value)}
            />
          </Field>
        </Modal>
      )}

      {tracks && (
        <Modal
          title={`轨道 ${tracks.gid}/${tracks.cid}`}
          onClose={() => setTracks(null)}
          footer={
            <button className="ghost" onClick={() => setTracks(null)}>
              关闭
            </button>
          }
        >
          <pre className="mono" style={{ whiteSpace: 'pre-wrap' }}>
            {tracks.body}
          </pre>
        </Modal>
      )}

      {removing && (
        <Confirm
          title={removing.kind === 'group' ? '删除分组' : '删除频道'}
          text={`确认删除「${removing.label}」？此操作不可撤销。`}
          onCancel={() => setRemoving(null)}
          onConfirm={() => void doRemove()}
        />
      )}

      {node}
    </>
  );
}
