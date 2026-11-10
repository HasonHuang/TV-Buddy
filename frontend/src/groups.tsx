/** 分组与频道管理。
 *
 * 版式基准：`docs/design/reference/img/p02-groups.png`（列表）、
 * `m01-group-edit.png` 与 `m02-channel-edit.png`（弹窗）。
 *
 * 频道上的 `tvg_id` / `tvg_name` / `logo` 和 `resolver_*` 都存在配置的
 * 未知字段里（`Channel.extra`），后端已经消费它们，所以这里直接读写同名键。
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import { api, copyText, putConfig } from './api';
import { Icon } from './icons';
import {
  Badge,
  Confirm,
  Field,
  HoverTip,
  Modal,
  NumberBox,
  PageHead,
  Select,
  Switch,
  useToast,
} from './ui';

export type Channel = {
  id: string;
  name: string;
  url: string;
  /** 来源订阅源 id；空表示本地频道。 */
  source_id?: string;
  source_key?: string;
  /** 跟随来源字段。缺失按 true 处理。 */
  sync_follow?: boolean;
  /** 本地回看（`常驻` 徽章的来源）。 */
  livestream?: boolean;
  /** 探测失败下线；缺省继承分组。 */
  offline_on_fail?: boolean;
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
  /** `manual` 或 `sync`。 */
  origin?: string;
  source_id?: string;
  source_group?: string;
  /** 分组级「探测失败下线」。频道可单独覆盖。 */
  offline_on_fail?: boolean;
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

const MODES: { value: string; code: string; label: string; desc: string }[] = [
  { value: 'proxy', code: 'PROXY', label: '服务器转发', desc: '视频经过本机。兼容性最好。' },
  { value: 'remux', code: 'REMUX', label: '服务器转封装', desc: '本机重新打包视频流。占用 CPU。' },
  { value: 'dash', code: 'DASH', label: 'DASH 转发', desc: '处理 DASH 清单。源必须提供 MPD。' },
  { value: 'redirect', code: '302', label: '跳转源地址', desc: '播放器直接连源。会显示源地址。' },
  { value: 'rewrite', code: 'REWRITE', label: '改写地址', desc: '本机改写清单地址，播放器再连源。' },
  { value: 'passthrough', code: 'PASSTHROUGH', label: '原样直通', desc: '直接使用源地址。不经过本机。' },
];

const ACCESS: { value: string; label: string; desc: string }[] = [
  { value: 'public', label: '公开', desc: '正常出现在订阅里。' },
  { value: 'archived', label: '归档', desc: '停播：不出现在订阅里，也不能播放。' },
];

const FORMATS: { value: string; label: string; desc: string }[] = [
  { value: 'hls', label: 'HLS', desc: '下发 m3u8 清单。' },
  { value: 'mpd', label: 'DASH', desc: '下发 mpd 清单。' },
];

/** 频道跟随源的规则说明。徽章与编辑表单共用。 */
const FOLLOW_HINT =
  '每次同步会用来源里的名称、地址和台标覆盖这个频道。手动改过这些字段后，频道停止跟随，之后的同步不再覆盖。';
const FIXED_HINT = '已停止跟随。同步不改名称、地址和台标。来源删除这个频道时，也会保留。';

/** 分组标题栏里的控件。点击这些控件不切换折叠。 */
const GROUP_HEAD_CONTROLS = 'button, input, select, textarea, a, .grip, .hover-tip';

/** 频道行里的操作控件。点击这些控件不切换选中。 */
const CHANNEL_ROW_CONTROLS = 'button, input, select, textarea, a, .grip';

/** 事件起点或终点落在控件上时返回 true。 */
function hitsControl(node: EventTarget | null): boolean {
  return node instanceof Element && node.closest(GROUP_HEAD_CONTROLS) !== null;
}

function hitsChannelControl(node: EventTarget | null): boolean {
  return node instanceof Element && node.closest(CHANNEL_ROW_CONTROLS) !== null;
}

/** 直接切换拖动落点样式，避免拖动经过每一行都触发 React 重渲染。 */
function highlightDrop(ref: { current: HTMLElement | null }, el: HTMLElement | null) {
  if (ref.current === el) return;
  ref.current?.classList.remove('dragover');
  ref.current = el;
  el?.classList.add('dragover');
}

/** 模式徽标的短标识，如 `PROXY`、`302`。 */
function modeCode(mode: string): string {
  return MODES.find((item) => item.value === mode)?.code ?? mode.toUpperCase();
}

/** 中文名加短标识，如「服务器转发（PROXY）」。 */
function modeText(mode: string): string {
  const item = MODES.find((entry) => entry.value === mode);
  if (!item) return mode;
  return item.label.includes(item.code) ? item.label : `${item.label}（${item.code}）`;
}

/** 徽标的悬停说明。 */
function modeTip(mode: string): string {
  const item = MODES.find((entry) => entry.value === mode);
  return item ? `${modeText(mode)}：${item.desc}` : mode;
}

/** 播放地址后缀由分组格式决定（S-URL-03）。 */
function entryExt(group: Group): string {
  return group.format === 'mpd' ? 'mpd' : 'm3u8';
}

function entryUrl(group: Group, channel: Channel): string {
  return `${location.origin}/${group.id}/${channel.id}.${entryExt(group)}`;
}

/**
 * 播放地址。附上当前账号的播放凭证。
 *
 * 与订阅里的地址同规则（`src/playlist.rs::entry_url`）。
 */
function playUrl(group: Group, channel: Channel, token: string): string {
  if (token === '') return entryUrl(group, channel);
  return `${location.origin}/l/${token}/${group.id}/${channel.id}.${entryExt(group)}`;
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
  offlineOnFail: boolean;
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
  /** `''` = 跟随分组；`on` / `off` = 显式覆盖。 */
  offlineOnFail: '' | 'on' | 'off';
  headers: string;
  proxy: ProxyForm;
  /** 来源源 id；空表示本地频道。 */
  sourceId: string;
  syncFollow: boolean;
  /** 打开弹窗时的跟随状态。用来判断用户是否显式切换。 */
  originalFollow: boolean;
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

const PREFETCH_OPTIONS: { value: PrefetchForm['state']; label: string; desc: string }[] = [
  { value: '', label: '跟随全局', desc: '使用上一级的预取设置。' },
  { value: 'inherit', label: '只覆盖数量与并发', desc: '开关仍跟随上一级。' },
  { value: 'on', label: '开启预取', desc: '为这个分组开启预取。' },
  { value: 'off', label: '关闭预取', desc: '为这个分组关闭预取。' },
];

const OFFLINE_OPTIONS: { value: ChannelForm['offlineOnFail']; label: string; desc: string }[] = [
  { value: '', label: '跟随分组', desc: '使用分组里的「探测失败下线」设置。' },
  { value: 'on', label: '开启', desc: '探测失败后从订阅清单里移除。' },
  { value: 'off', label: '关闭', desc: '始终留在订阅清单里。' },
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
        <Field label="代理类型" tip="读取频道时使用的代理协议。代理地址留空时不启用代理。">
          <Select
            ariaLabel="代理类型"
            value={proxy.kind}
            options={[
              { value: 'http', label: 'HTTP', desc: '普通 HTTP 代理。' },
              { value: 'https', label: 'HTTPS', desc: '加密的 HTTP 代理。' },
              { value: 'socks5', label: 'SOCKS5', desc: 'SOCKS5 代理。' },
            ]}
            onChange={(next) => onProxy({ ...proxy, kind: next })}
            compact
          />
        </Field>
        <Field label="代理地址" tip="读取频道时经过的代理。留空表示直接连接。">
          <input
            placeholder="127.0.0.1:8080，留空表示不用"
            value={proxy.addr}
            onChange={(e) => onProxy({ ...proxy, addr: e.target.value })}
          />
        </Field>
        <Field label="代理用户名" tip="代理需要认证时使用。">
          <input
            value={proxy.user}
            autoComplete="off"
            onChange={(e) => onProxy({ ...proxy, user: e.target.value })}
          />
        </Field>
        <Field label="代理密码" tip="代理需要认证时使用。密码只保存在本地数据库。">
          <input
            type="password"
            autoComplete="new-password"
            value={proxy.pass}
            onChange={(e) => onProxy({ ...proxy, pass: e.target.value })}
          />
        </Field>
      </div>
      <Field
        label="请求头"
        tip="读取频道时附加的请求头。源需要额外的身份信息时填写。"
        hint="每行一条，格式为「名称: 值」。留空表示不加。"
      >
        <textarea
          rows={3}
          placeholder="User-Agent: VLC/3.0"
          value={headers}
          onChange={(e) => onHeaders(e.target.value)}
        />
      </Field>
      <Field
        label="预取覆盖"
        tip="覆盖上一级的分组或全局预取设置。留空表示跟随上一级。"
        hint="数值留空表示不覆盖。"
      >
        <Select
          ariaLabel="预取覆盖"
          value={prefetch.state}
          options={PREFETCH_OPTIONS}
          onChange={(next) =>
            onPrefetch({ ...prefetch, state: next as PrefetchForm['state'] })
          }
          width={190}
        />
      </Field>
      {numeric && (
        <div className="fields">
          <Field label="单次上限（0 = 不限）" tip="每个清单最多预取的切片数。0 表示抓满清单窗口。">
            <NumberBox
              min={0}
              value={prefetch.limit}
              placeholder="留空不覆盖"
              onChange={(next) => onPrefetch({ ...prefetch, limit: next })}
            />
          </Field>
          <Field label="并发数（1-32）" tip="同时发出的预取请求数。过高会挤占带宽。">
            <NumberBox
              min={1}
              max={32}
              value={prefetch.concurrency}
              placeholder="留空不覆盖"
              onChange={(next) => onPrefetch({ ...prefetch, concurrency: next })}
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
  const [confirmBatch, setConfirmBatch] = useState(false);
  const [removing, setRemoving] = useState<
    {
      kind: 'group' | 'channel';
      gid: string;
      cid?: string;
      label: string;
      synced?: boolean;
    } | null
  >(null);
  const [importing, setImporting] = useState<{ kind: 'json' | 'm3u'; force: boolean } | null>(null);
  const [importText, setImportText] = useState('');
  const [m3uMode, setM3uMode] = useState<'append' | 'merge' | 'replace'>('append');
  const [health, setHealth] = useState<Record<string, HealthEntry>>({});
  /** `[health].enabled`。关闭时不按探测结果下线。 */
  const [healthEnabled, setHealthEnabled] = useState(true);
  const [tracks, setTracks] = useState<{ gid: string; cid: string; body: string } | null>(null);
  /** 多选：批量删除与批量导出。 */
  const [multi, setMulti] = useState(false);
  const [selected, setSelected] = useState<string[]>([]);
  /** 选中的频道。分组单独记录，删除时再合并。 */
  const [selectedChannels, setSelectedChannels] = useState<{ gid: string; cid: string }[]>([]);
  /** 已折叠的分组 id。只影响本页显示，不写回配置。 */
  const [collapsed, setCollapsed] = useState<string[]>([]);
  /** 正在拖动的分组 id。为空表示没有拖动。 */
  const [dragging, setDragging] = useState('');
  /** 正在拖动的频道。只允许在同一分组内排序。 */
  const [draggingChannel, setDraggingChannel] = useState<{ gid: string; cid: string } | null>(
    null,
  );
  /** 拖动落点的 DOM 引用。直接用 class 画高亮，不走 state。 */
  const groupDropRef = useRef<HTMLElement | null>(null);
  const channelDropRef = useRef<HTMLElement | null>(null);
  /** 标题栏按下时的元素。按下与松开不在同一元素时，click 落在共同祖先上。 */
  const headPress = useRef<EventTarget | null>(null);
  /** 解析脚本列表；打开频道弹窗时按需加载。 */
  const [scripts, setScripts] = useState<string[]>([]);
  /** 当前账号的播放凭证。复制播放地址时附上，否则在开启凭证校验时是死链。 */
  const [playToken, setPlayToken] = useState('');
  /** 来源 id → 源名称。分组与频道徽章用。 */
  const [sourceNames, setSourceNames] = useState<Record<string, string>>({});

  useEffect(() => {
    void api<{ token?: string }>('GET', '/me')
      .then((body) => setPlayToken(body.token ?? ''))
      .catch(() => setPlayToken(''));
  }, []);

  const load = useCallback(async () => {
    try {
      setGroups(await api<Group[]>('GET', '/groups/export'));
      // 逐频道探测结果；未启用探测时为空对象。
      const body = await api<{ enabled?: boolean; entries?: Record<string, HealthEntry> }>(
        'GET',
        '/health',
      );
      setHealthEnabled(body.enabled !== false);
      setHealth(body.entries ?? {});
      const playlist = await api<{ sources?: { id: string; name: string }[] }>(
        'GET',
        '/playlist',
      ).catch(() => ({ sources: [] }));
      setSourceNames(
        Object.fromEntries(
          (playlist.sources ?? []).map((source) => [source.id, source.name || source.id]),
        ),
      );
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  // 分组或频道被单独删除后，清掉已失效的选中项。
  useEffect(() => {
    const groupIds = new Set(groups.map((group) => group.id));
    const channelIds = new Map(
      groups.map((group) => [
        group.id,
        new Set((group.channels ?? []).map((channel) => channel.id)),
      ]),
    );
    setSelected((current) => {
      const next = current.filter((gid) => groupIds.has(gid));
      return next.length === current.length ? current : next;
    });
    setSelectedChannels((current) => {
      const next = current.filter((item) => channelIds.get(item.gid)?.has(item.cid) ?? false);
      return next.length === current.length ? current : next;
    });
  }, [groups]);

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
      offline_on_fail: form.offlineOnFail,
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
      // `null` 清空覆盖，回到跟随分组。
      offline_on_fail: form.offlineOnFail === '' ? null : form.offlineOnFail === 'on',
      headers: textToHeaders(form.headers),
      proxy: proxyPayload(form.proxy),
      prefetch: prefetchPayload(prefetch),
    };
    // 只在用户显式切换时发送；否则由服务端按来源字段变化自动固定。
    if (!isNew && form.syncFollow !== form.originalFollow) {
      payload.sync_follow = form.syncFollow;
    }
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

  const detach = async (kind: 'group' | 'channel', gid: string, cid?: string) => {
    try {
      await api(
        'POST',
        kind === 'group'
          ? `/groups/${gid}/detach`
          : `/groups/${gid}/channels/${cid}/detach`,
      );
      ok(kind === 'group' ? '分组已断开订阅' : '频道已断开订阅');
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
      await putConfig({ ...config, groups: list });
      // 顺序只影响分组列表；直接更新本地状态，不必重拉探测与播放列表。
      setGroups(list);
      ok('顺序已保存');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
      // 保存冲突或失败时刷新，避免继续拿旧快照操作。
      await load();
    }
  };

  /** 按 id 上移或下移分组，避免使用渲染快照里的下标。 */
  const moveGroup = (gid: string, delta: number) =>
    reorder((list) => {
      const from = list.findIndex((item) => item.id === gid);
      if (from < 0) return false;
      const to = from + delta;
      if (to < 0 || to >= list.length) return false;
      const [moved] = list.splice(from, 1);
      list.splice(to, 0, moved);
      return true;
    });

  /** 拖动排序：把分组 `gid` 移到目标分组 `targetGid` 的位置。 */
  const moveGroupTo = (gid: string, targetGid: string) =>
    reorder((list) => {
      const from = list.findIndex((item) => item.id === gid);
      const to = list.findIndex((item) => item.id === targetGid);
      if (from < 0 || to < 0 || from === to) return false;
      const [moved] = list.splice(from, 1);
      list.splice(to, 0, moved);
      return true;
    });

  /** 在最新配置里按 id 找频道，避免使用渲染快照里的下标。 */
  const moveChannel = (gid: string, cid: string, delta: number) =>
    reorder((list) => {
      const channels = list.find((item) => item.id === gid)?.channels;
      if (!channels) return false;
      const from = channels.findIndex((item) => item.id === cid);
      if (from < 0) return false;
      const to = from + delta;
      if (to < 0 || to >= channels.length) return false;
      const [moved] = channels.splice(from, 1);
      channels.splice(to, 0, moved);
      return true;
    });

  /** 把频道移到目标频道的位置；`targetCid` 为空表示移到第一位。 */
  const moveChannelTo = (gid: string, cid: string, targetCid: string | null) =>
    reorder((list) => {
      const channels = list.find((item) => item.id === gid)?.channels;
      if (!channels) return false;
      const from = channels.findIndex((item) => item.id === cid);
      if (from < 0) return false;
      if (targetCid === null) {
        if (from === 0) return false;
        const [moved] = channels.splice(from, 1);
        channels.unshift(moved);
        return true;
      }
      const to = channels.findIndex((item) => item.id === targetCid);
      if (to < 0 || from === to) return false;
      const [moved] = channels.splice(from, 1);
      channels.splice(to, 0, moved);
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

  const toggleChannelSelected = (gid: string, cid: string) => {
    setSelectedChannels((current) =>
      current.some((item) => item.gid === gid && item.cid === cid)
        ? current.filter((item) => !(item.gid === gid && item.cid === cid))
        : [...current, { gid, cid }],
    );
  };

  const isChannelSelected = (gid: string, cid: string) =>
    selectedChannels.some((item) => item.gid === gid && item.cid === cid);

  /** 折叠或展开一个分组的频道列表。 */
  const toggleCollapsed = (gid: string) => {
    setCollapsed((current) =>
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

  /** 批量删除：先删频道，再删分组。分组内的频道随分组一起删除。 */
  const removeSelected = async () => {
    const groupIds = selected;
    const channels = selectedChannels.filter((item) => !groupIds.includes(item.gid));
    if (groupIds.length === 0 && channels.length === 0) return;
    try {
      for (const item of channels) {
        await api('DELETE', `/groups/${item.gid}/channels/${item.cid}`);
      }
      for (const gid of groupIds) {
        await api('DELETE', `/groups/${gid}`);
      }
      const removed = [
        groupIds.length > 0 ? `${groupIds.length} 个分组` : '',
        channels.length > 0 ? `${channels.length} 个频道` : '',
      ]
        .filter(Boolean)
        .join('、');
      ok(`已删除 ${removed}`);
      setSelected([]);
      setSelectedChannels([]);
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
            offlineOnFail: group.offline_on_fail === true,
          }
        : {
            id: '',
            name: '',
            mode: 'proxy',
            access: 'public',
            format: 'hls',
            headers: '',
            proxy: emptyProxy(),
            offlineOnFail: false,
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
            offlineOnFail:
              channel.offline_on_fail === true
                ? 'on'
                : channel.offline_on_fail === false
                  ? 'off'
                  : '',
            headers: headersToText(channel.headers),
            proxy: proxyForm(channel.proxy),
            sourceId: channel.source_id ?? '',
            syncFollow: channel.sync_follow !== false,
            originalFollow: channel.sync_follow !== false,
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
            offlineOnFail: '',
            headers: '',
            proxy: emptyProxy(),
            sourceId: '',
            syncFollow: true,
            originalFollow: true,
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
  const selectedLooseChannels = selectedChannels.filter((item) => !selected.includes(item.gid));
  const selectionText = [
    selected.length > 0 ? `${selected.length} 个分组` : '',
    selectedChannels.length > 0 ? `${selectedChannels.length} 个频道` : '',
  ]
    .filter(Boolean)
    .join('、');

  return (
    <>
      <PageHead icon="groups" title="分组管理" sub="管理直播分组与频道">
        <input
          className="search"
          aria-label="搜索分组或频道"
          title="按分组名称、分组 ID 或频道名称筛选"
          placeholder="搜索分组或频道"
          value={filter}
          onChange={(event) => setFilter(event.target.value)}
        />
        <button
          className="ghost"
          title="从 M3U 文本导入分组与频道"
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
          title="从 JSON 追加分组；同 ID 的分组会跳过"
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
          title="从 JSON 覆盖分组；同 ID 的分组会整体替换"
          onClick={() => {
            setImporting({ kind: 'json', force: true });
            setImportText('');
          }}
        >
          <Icon name="folder" size={14} />
          JSON 覆盖
        </button>
        <button
          className="ghost"
          title="导出全部分组与频道为 JSON"
          onClick={() => void exportAll()}
        >
          <Icon name="download" size={14} />
          导出全部
        </button>
        <button
          className={multi ? '' : 'ghost'}
          title="进入批量选择模式，可批量删除分组或频道"
          onClick={() => {
            setMulti(!multi);
            setSelected([]);
            setSelectedChannels([]);
          }}
        >
          <Icon name="check" size={14} />
          多选
        </button>
        <button title="新建一个分组" onClick={() => openGroup()}>
          <Icon name="plus" size={14} />
          新建分组
        </button>
      </PageHead>

      {multi && (selected.length > 0 || selectedChannels.length > 0) && (
        <div className="card">
          <div className="row between">
            <span>已选 {selectionText}</span>
            <div className="row">
              {selected.length > 0 && (
                <button
                  className="ghost"
                  title="导出已选分组为 JSON"
                  onClick={() => void exportSelected()}
                >
                  批量导出
                </button>
              )}
              <button
                className="ghost danger-text"
                title="删除已选分组与频道"
                onClick={() => setConfirmBatch(true)}
              >
                批量删除
              </button>
            </div>
          </div>
        </div>
      )}

      {visible.length === 0 && (
        <div className="card muted">
          {groups.length === 0
            ? '还没有分组。点「新建分组」创建，或用 M3U 导入现有清单。'
            : '没有匹配的分组。'}
        </div>
      )}

      {visible.map((group) => {
        const channels = group.channels ?? [];
        // 排序按 `groups` 的绝对下标；搜索过滤时也不会错位。
        const groupIndex = groups.indexOf(group);
        const isCollapsed = collapsed.includes(group.id);
        return (
          <div
            className="card flush group-card"
            key={group.id}
            onDragOver={(event) => {
              if (dragging === '') return;
              event.preventDefault();
              if (dragging === group.id) {
                highlightDrop(groupDropRef, null);
                return;
              }
              highlightDrop(groupDropRef, event.currentTarget);
            }}
            onDragLeave={(event) => {
              if (event.currentTarget.contains(event.relatedTarget as Node | null)) {
                return;
              }
              if (groupDropRef.current === event.currentTarget) {
                highlightDrop(groupDropRef, null);
              }
            }}
            onDrop={(event) => {
              event.preventDefault();
              if (dragging !== '' && dragging !== group.id) {
                void moveGroupTo(dragging, group.id);
              }
              setDragging('');
              highlightDrop(groupDropRef, null);
            }}
          >
            <div
              className="group-head"
              onPointerDown={(event) => {
                headPress.current = event.target;
              }}
              onClick={(event) => {
                // 只响应空白处与标题。控件和悬停说明保持原行为。
                // 按下与松开跨元素时，click 的目标是共同祖先，所以要同时看起点。
                const pressed = headPress.current;
                headPress.current = null;
                if (hitsControl(event.target) || hitsControl(pressed)) {
                  return;
                }
                toggleCollapsed(group.id);
              }}
            >
              <span
                className="grip"
                draggable
                title="拖动排序"
                onDragStart={(event) => {
                  setDragging(group.id);
                  event.dataTransfer.effectAllowed = 'move';
                  // Firefox 需要写入数据才会开始拖动。
                  event.dataTransfer.setData('text/plain', group.id);
                }}
                onDragEnd={() => {
                  setDragging('');
                  highlightDrop(groupDropRef, null);
                }}
              >
                <Icon name="grip" size={14} />
              </span>
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
              {group.source_id ? (
                <HoverTip text="这个分组由托管源同步生成。断开订阅后，分组和频道保留为本地内容。">
                  <Badge tone="info">
                    来源：{sourceNames[group.source_id] ?? group.source_id}
                  </Badge>
                </HoverTip>
              ) : (
                <Badge tone="muted">本地</Badge>
              )}
              {group.offline_on_fail === true && (
                <HoverTip text="组内探测失败的频道不进订阅。频道可以单独覆盖。">
                  <Badge tone="warn">失败下线</Badge>
                </HoverTip>
              )}
              <div className="ops">
                <HoverTip text={modeTip(group.mode)}>
                  <span className={`badge mode ${group.mode}`}>{modeCode(group.mode)}</span>
                </HoverTip>
                <span className="count">{channels.length} 频道</span>
                <button
                  className="icon-btn move"
                  disabled={groupIndex <= 0}
                  title="上移分组"
                  onClick={() => void moveGroup(group.id, -1)}
                >
                  <Icon name="up" size={12} />
                </button>
                <button
                  className="icon-btn move"
                  disabled={groupIndex < 0 || groupIndex >= groups.length - 1}
                  title="下移分组"
                  onClick={() => void moveGroup(group.id, 1)}
                >
                  <Icon name="down" size={12} />
                </button>
                <button
                  className="chip"
                  aria-label="编辑分组"
                  onClick={() => openGroup(group)}
                >
                  编辑
                </button>
                <button
                  className="chip icon-only"
                  title="导出该分组的 JSON"
                  onClick={() => void exportGroup(group.id)}
                >
                  <Icon name="download" size={13} />
                </button>
                {group.source_id && (
                  <button
                    className="chip"
                    title="断开来源关系，内容保留"
                    onClick={() => void detach('group', group.id)}
                  >
                    断开订阅
                  </button>
                )}
                <button
                  className="chip danger-text"
                  aria-label="删除分组"
                  onClick={() =>
                    setRemoving({ kind: 'group', gid: group.id, label: group.name || group.id })
                  }
                >
                  删除
                </button>
                <button
                  className="chip primary"
                  aria-label="新增频道"
                  onClick={() => openChannel(group.id)}
                >
                  <Icon name="plus" size={13} />
                  频道
                </button>
                <button
                  className="icon-btn chev"
                  title={isCollapsed ? '展开分组' : '折叠分组'}
                  onClick={() => toggleCollapsed(group.id)}
                >
                  <Icon name="down" size={13} className={isCollapsed ? 'folded' : ''} />
                </button>
              </div>
            </div>

            {!isCollapsed &&
              channels.map((channel, channelIndex) => {
              const state = health[`${group.id}/${channel.id}`];
              const offlineOnFail =
                channel.offline_on_fail ?? (group.offline_on_fail === true);
              const offline = healthEnabled && offlineOnFail && state?.ok === false;
              const logo = text(channel, 'logo') || text(channel, 'tvg_logo');
              const tvgId = text(channel, 'tvg_id');
              const allTracks = group.mode === 'dash' && (channel.selected_tracks ?? []).length === 0;
              const local =
                group.mode === 'passthrough'
                  ? channel.url
                  : playUrl(group, channel, playToken);
              return (
                <div
                  className={`ch-row${multi ? ' selectable' : ''}${
                    isChannelSelected(group.id, channel.id) ? ' selected' : ''
                  }`}
                  key={channel.id}
                  onClickCapture={(event) => {
                    if (!multi) return;
                    if (hitsChannelControl(event.target)) return;
                    toggleChannelSelected(group.id, channel.id);
                  }}
                  onDragOver={(event) => {
                    if (draggingChannel?.gid !== group.id) return;
                    if (draggingChannel.cid === channel.id) return;
                    event.preventDefault();
                    highlightDrop(channelDropRef, event.currentTarget);
                  }}
                  onDragLeave={(event) => {
                    if (event.currentTarget.contains(event.relatedTarget as Node | null)) {
                      return;
                    }
                    if (channelDropRef.current === event.currentTarget) {
                      highlightDrop(channelDropRef, null);
                    }
                  }}
                  onDrop={(event) => {
                    // 分组拖动交给外层分组卡处理；这里只接频道拖动。
                    if (!draggingChannel) return;
                    event.preventDefault();
                    if (draggingChannel.gid === group.id) {
                      void moveChannelTo(group.id, draggingChannel.cid, channel.id);
                    }
                    setDraggingChannel(null);
                    highlightDrop(channelDropRef, null);
                  }}
                >
                  <span
                    className="grip"
                    draggable
                    title="拖动排序"
                    onDragStart={(event) => {
                      setDraggingChannel({ gid: group.id, cid: channel.id });
                      highlightDrop(channelDropRef, null);
                      event.dataTransfer.effectAllowed = 'move';
                      // Firefox 需要写入数据才会开始拖动。
                      event.dataTransfer.setData('text/plain', channel.id);
                    }}
                    onDragEnd={() => {
                      setDraggingChannel(null);
                      highlightDrop(channelDropRef, null);
                    }}
                  >
                    <Icon name="grip" size={13} />
                  </span>
                  {multi && (
                    <input
                      className="ch-select"
                      type="checkbox"
                      title="选择频道"
                      aria-label={`选择频道 ${channel.name || channel.id}`}
                      checked={isChannelSelected(group.id, channel.id)}
                      onChange={() => toggleChannelSelected(group.id, channel.id)}
                    />
                  )}
                  <span
                    className={`dot${state?.ok === true ? ' ok' : state?.ok === false ? ' err' : ''}`}
                    title={state ? (state.ok ? '最近探测成功' : '最近探测失败') : '未启用主动探测'}
                  />
                  <div className="ch-main">
                    <div className="ch-name">
                      <span>{channel.name || channel.id}</span>
                      <span className="cid">[{channel.id}]</span>
                      {state?.ok === false && <Badge tone="warn">异常</Badge>}
                      {offline && (
                        <HoverTip text="最近一次探测失败，已从订阅清单移除。探测成功后自动恢复。">
                          <Badge tone="danger">已下线</Badge>
                        </HoverTip>
                      )}
                      {channel.livestream === true && <Badge tone="info">回看</Badge>}
                      {allTracks && <Badge tone="muted">全部轨道</Badge>}
                    </div>
                    <div className="ch-url">{channel.url}</div>
                    <div className="ch-url local">
                      {local}
                      {group.mode === 'passthrough' && <Badge tone="muted">直通</Badge>}
                    </div>
                    <div className="ch-badges">
                      {channel.source_id ? (
                        <HoverTip
                          text={
                            channel.sync_follow === false
                              ? FIXED_HINT
                              : `正在跟随来源。${FOLLOW_HINT}`
                          }
                        >
                          <Badge tone={channel.sync_follow === false ? 'warn' : 'info'}>
                            来源：{sourceNames[channel.source_id] ?? channel.source_id}
                            {channel.sync_follow === false ? ' · 固定' : ' · 跟随'}
                          </Badge>
                        </HoverTip>
                      ) : (
                        <Badge tone="muted">本地</Badge>
                      )}
                      {tvgId !== '' && <Badge tone="muted">tvg-id</Badge>}
                      {logo !== '' && <Badge tone="muted">Logo</Badge>}
                      <PrefetchBadge raw={channel.prefetch} />
                    </div>
                  </div>
                  <div className="ch-ops">
                    <button
                      className="icon-btn move pin-top"
                      disabled={channelIndex === 0}
                      title="置顶频道"
                      aria-label="置顶频道"
                      onClick={() => void moveChannelTo(group.id, channel.id, null)}
                    >
                      <Icon name="pin" size={12} />
                    </button>
                    <button
                      className="icon-btn move"
                      disabled={channelIndex === 0}
                      title="上移频道"
                      onClick={() => void moveChannel(group.id, channel.id, -1)}
                    >
                      <Icon name="up" size={12} />
                    </button>
                    <button
                      className="icon-btn move"
                      disabled={channelIndex >= channels.length - 1}
                      title="下移频道"
                      onClick={() => void moveChannel(group.id, channel.id, 1)}
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
                    <button
                      className="chip"
                      aria-label="编辑频道"
                      onClick={() => openChannel(group.id, channel)}
                    >
                      编辑
                    </button>
                    {channel.source_id && (
                      <button
                        className="chip"
                        title="断开来源关系，内容保留"
                        onClick={() => void detach('channel', group.id, channel.id)}
                      >
                        断开订阅
                      </button>
                    )}
                    <button
                      className="chip danger-text"
                      aria-label="删除频道"
                      onClick={() =>
                        setRemoving({
                          kind: 'channel',
                          gid: group.id,
                          cid: channel.id,
                          label: channel.name || channel.id,
                          synced: Boolean(channel.source_id),
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
          <Field
            label="分组 ID"
            tip="用于订阅地址。建成后不能修改。"
            hint="建成后不能改。"
          >
            <input
              value={groupForm.form.id}
              disabled={!groupForm.isNew}
              placeholder="如 cctv"
              onChange={(event) =>
                setGroupForm({ ...groupForm, form: { ...groupForm.form, id: event.target.value } })
              }
            />
          </Field>
          <Field label="显示名称" tip="面板与播放器里显示的分组名称。">
            <input
              value={groupForm.form.name}
              placeholder="如 央视频道"
              onChange={(event) =>
                setGroupForm({ ...groupForm, form: { ...groupForm.form, name: event.target.value } })
              }
            />
          </Field>
          <div className="fields">
            <Field
              label="工作模式"
              tip="PROXY 兼容性最好。302 与 PASSTHROUGH 由播放器直连源，不经过本机。"
            >
              <Select
                ariaLabel="工作模式"
                value={groupForm.form.mode}
                options={MODES.map((mode) => ({
                  value: mode.value,
                  label: modeText(mode.value),
                  desc: mode.desc,
                }))}
                onChange={(next) =>
                  setGroupForm({
                    ...groupForm,
                    form: { ...groupForm.form, mode: next },
                  })
                }
                width={200}
              />
            </Field>
            <Field label="内容格式" tip="选择下发给播放器的清单格式。">
              <Select
                ariaLabel="内容格式"
                value={groupForm.form.format}
                options={FORMATS}
                onChange={(next) =>
                  setGroupForm({
                    ...groupForm,
                    form: { ...groupForm.form, format: next },
                  })
                }
                width={160}
              />
            </Field>
          </div>
          <Field
            label="访问范围"
            tip="归档的分组停播：不出现在订阅里，也不能播放。"
          >
            <Select
              ariaLabel="访问范围"
              value={groupForm.form.access}
              options={ACCESS}
              onChange={(next) =>
                setGroupForm({
                  ...groupForm,
                  form: { ...groupForm.form, access: next },
                })
              }
              width={200}
            />
          </Field>
          <Field
            label="探测失败下线"
            tip="开启后，组内最近一次探测失败的频道不出现在订阅里。探测成功后自动恢复。频道可以单独覆盖。"
            hint="默认关闭。需要启用健康探测。"
          >
            <Switch
              label="探测失败下线"
              checked={groupForm.form.offlineOnFail}
              onChange={(offlineOnFail) =>
                setGroupForm({
                  ...groupForm,
                  form: { ...groupForm.form, offlineOnFail },
                })
              }
            />
          </Field>
          <NetworkBlock
            summary="网络配置 · 分组默认值，频道可以单独覆盖"
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
            <Field label="频道 ID" tip="用于订阅地址。建成后不能修改。" hint="建成后不能改。">
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
            <Field label="频道名称" tip="面板与播放器里显示的频道名称。">
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
          {channelForm.form.sourceId !== '' && (
            <div className="fields">
              <Field
                label="订阅来源"
                tip="开启时，同步用来源覆盖名称、地址和台标。关闭时保留当前值。"
                hint={
                  channelForm.form.syncFollow
                    ? FOLLOW_HINT
                    : `${FIXED_HINT} 勾选后恢复跟随。`
                }
              >
                <div className="row">
                  <Badge tone="info">
                    来源：{sourceNames[channelForm.form.sourceId] ?? channelForm.form.sourceId}
                  </Badge>
                  <label>
                    <input
                      type="checkbox"
                      checked={channelForm.form.syncFollow}
                      onChange={(event) =>
                        setChannelForm({
                          ...channelForm,
                          form: { ...channelForm.form, syncFollow: event.target.checked },
                        })
                      }
                    />{' '}
                    跟随订阅源
                  </label>
                </div>
              </Field>
            </div>
          )}
          <div className="fields">
            <Field label="源地址" tip="频道原始地址。系统从这里读取清单与切片。">
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
            <Field label="频道台标地址" tip="播放器列表里显示的台标地址。可以填站内路径或外部图片地址。">
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
            <Field label="EPG 频道 ID" tip="用于匹配节目单的频道标识。留空时使用频道 ID。" hint="留空则同频道 ID。">
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
            <Field label="EPG 频道名称" tip="用于匹配节目单的频道名称。留空时使用频道名称。" hint="留空则同频道名称。">
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
          <div className="section-title">地址解析</div>
          <Field
            label="地址来源"
            tip="固定地址直接读取。解析脚本先运行脚本，再使用脚本的输出。"
          >
            <Select
              ariaLabel="地址来源"
              value={channelForm.form.upstream}
              options={[
                { value: 'static', label: '固定地址', desc: '直接读取下面的源地址。' },
                { value: 'script', label: '解析脚本', desc: '先运行脚本，再把输出当作源地址。' },
              ]}
              onChange={(next) =>
                setChannelForm({
                  ...channelForm,
                  form: {
                    ...channelForm.form,
                    upstream: next as 'static' | 'script',
                  },
                })
              }
              width={160}
            />
          </Field>
          {channelForm.form.upstream === 'script' && (
            <div className="fields">
              <Field
                label="脚本"
                tip="解析脚本放在缓存目录的 resolvers 文件夹。脚本的输出就是源地址。"
                hint="位于 <缓存目录>/resolvers/。"
              >
                <Select
                  ariaLabel="脚本"
                  value={channelForm.form.script}
                  options={[
                    { value: '', label: '（请选择）' },
                    ...scripts.map((name) => ({ value: name, label: name })),
                  ]}
                  onChange={(next) =>
                    setChannelForm({
                      ...channelForm,
                      form: { ...channelForm.form, script: next },
                    })
                  }
                />
              </Field>
              <Field label="脚本参数" tip="传给解析脚本的参数。不需要时留空。">
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
              <Field label="缓存秒数" tip="脚本结果的缓存时间。0 表示每次都重新运行脚本。">
                <NumberBox
                  min={0}
                  value={channelForm.form.scriptTtl}
                  onChange={(next) =>
                    setChannelForm({
                      ...channelForm,
                      form: { ...channelForm.form, scriptTtl: next },
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
                onClick={() => void copy(channelForm.form.url, '源地址')}
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
              <span className="muted">保留历史切片，可以回看过去的节目。</span>
              <Switch
                label="开启回看"
                checked={channelForm.form.livestream}
                onChange={(livestream) =>
                  setChannelForm({ ...channelForm, form: { ...channelForm.form, livestream } })
                }
              />
            </div>
            {channelForm.form.livestream && (
              <div className="fields">
                <Field label="保留小时数" tip="历史切片保留的时长。0 表示不按时间限制。" hint="0 表示不限制。">
                  <NumberBox
                    min={0}
                    value={channelForm.form.timeshiftHours}
                    onChange={(next) =>
                      setChannelForm({
                        ...channelForm,
                        form: { ...channelForm.form, timeshiftHours: next },
                      })
                    }
                  />
                </Field>
                <Field label="磁盘上限（GB）" hint="0 表示不限制。">
                  <NumberBox
                    min={0}
                    value={channelForm.form.timeshiftMaxGb}
                    onChange={(next) =>
                      setChannelForm({
                        ...channelForm,
                        form: { ...channelForm.form, timeshiftMaxGb: next },
                      })
                    }
                  />
                </Field>
              </div>
            )}
          </details>

          <hr className="divider" />
          <div className="section-title">探测下线</div>
          <Field
            label="探测失败下线"
            tip="开启后，最近一次探测失败时，这个频道不出现在订阅清单里。探测成功后自动恢复。"
            hint="默认跟随分组。需要启用健康探测。"
          >
            <Select
              ariaLabel="探测失败下线"
              value={channelForm.form.offlineOnFail}
              options={OFFLINE_OPTIONS}
              onChange={(next) =>
                setChannelForm({
                  ...channelForm,
                  form: {
                    ...channelForm.form,
                    offlineOnFail: next as ChannelForm['offlineOnFail'],
                  },
                })
              }
              width={200}
            />
          </Field>

          <NetworkBlock
            summary="网络配置 · 留空则使用分组配置"
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
              <button
                title="按当前文本导入分组"
                disabled={importText.trim() === ''}
                onClick={() => void submitImport()}
              >
                导入
              </button>
            </>
          }
        >
          {importing.kind === 'm3u' ? (
            <Field
              label="同名分组处理"
              tip="选择导入内容遇到同名分组时怎么做。"
            >
              <Select
                ariaLabel="M3U 导入模式"
                value={m3uMode}
                options={[
                  { value: 'append', label: '跳过同名分组', desc: '只导入新分组。' },
                  { value: 'merge', label: '补齐同名分组', desc: '保留原频道，只加入缺少的。' },
                  { value: 'replace', label: '替换同名分组', desc: '整组换成导入的内容。' },
                ]}
                onChange={(next) =>
                  setM3uMode(next as 'append' | 'merge' | 'replace')
                }
                width={260}
              />
            </Field>
          ) : (
            <p className="muted">
              {importing.force
                ? '按分组 ID 覆盖：已有的分组整体替换，其余新增。'
                : '按分组 ID 追加：已有的分组跳过，其余新增。'}
            </p>
          )}
          <Field
            label={importing.kind === 'm3u' ? 'M3U 文本' : '分组 JSON 数组'}
            tip="导入会新增或覆盖分组。重复的频道 ID 按所选方式处理。"
          >
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

      {confirmBatch ? (
        <Confirm
          title="批量删除选中项？"
          text={
            selected.length > 0 && selectedLooseChannels.length > 0
              ? `将删除选中的 ${selected.length} 个分组及其全部频道，以及 ${selectedLooseChannels.length} 个其他频道。这个操作不能撤销。`
              : selected.length > 0
                ? `将删除选中的 ${selected.length} 个分组及其全部频道。这个操作不能撤销。`
                : `将删除选中的 ${selectedLooseChannels.length} 个频道。这个操作不能撤销。`
          }
          confirmText="删除选中项"
          onCancel={() => setConfirmBatch(false)}
          onConfirm={() => {
            setConfirmBatch(false);
            void removeSelected();
          }}
        />
      ) : null}

      {removing && (
        <Confirm
          title={removing.kind === 'group' ? '删除分组' : '删除频道'}
          text={
            removing.synced
              ? `确认删除「${removing.label}」？此操作不可撤销。这个频道来自订阅源，删除后会加入排除列表，下次同步不会恢复。`
              : `确认删除「${removing.label}」？此操作不可撤销。`
          }
          onCancel={() => setRemoving(null)}
          onConfirm={() => void doRemove()}
        />
      )}

      {node}
    </>
  );
}
