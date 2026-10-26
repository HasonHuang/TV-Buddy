/** 分组与频道管理（M1）。 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { Badge, Confirm, Modal, useToast } from './ui';

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

const MODES = ['proxy', 'remux', 'dash', 'redirect', 'rewrite', 'passthrough'];
const ACCESS = ['public', 'hidden', 'open', 'archived'];
const ACCESS_TONE: Record<string, string> = {
  public: 'ok',
  hidden: 'warn',
  open: 'info',
  archived: 'muted',
};

/** 播放地址后缀由分组格式决定（S-URL-03）。 */
function entryExt(group: Group): string {
  return group.format === 'mpd' ? 'mpd' : 'm3u8';
}

function entryUrl(group: Group, channel: Channel): string {
  return `${location.origin}/${group.id}/${channel.id}.${entryExt(group)}`;
}

type GroupForm = {
  id: string;
  name: string;
  mode: string;
  access: string;
  prefetch: PrefetchForm;
};
type ChannelForm = { id: string; name: string; url: string; prefetch: PrefetchForm };

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

function emptyPrefetch(): PrefetchForm {
  return { state: '', limit: '', concurrency: '' };
}

/** 表单 → 后端 `prefetch` 字段；`null` 表示清空覆盖。 */
function prefetchPayload(form: PrefetchForm): Record<string, unknown> | null {
  if (form.state === '') {
    return null;
  }
  const payload: Record<string, unknown> = {};
  if (form.state !== 'inherit') {
    payload.enabled = form.state === 'on';
  }
  const limit = form.limit.trim();
  if (limit !== '') {
    payload.prefetch_limit = Number(limit);
  }
  const concurrency = form.concurrency.trim();
  if (concurrency !== '') {
    payload.concurrency = Number(concurrency);
  }
  return payload;
}

/** 后端 `prefetch` 字段 → 表单。 */
function prefetchForm(raw: unknown): PrefetchForm {
  if (!raw || typeof raw !== 'object') {
    return emptyPrefetch();
  }
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
  if (!raw || typeof raw !== 'object') {
    return null;
  }
  const form = prefetchForm(raw);
  const label =
    form.state === 'off'
      ? '预取关闭'
      : form.state === 'on'
        ? '预取开启'
        : `预取 ${form.limit || '—'} 片 / ${form.concurrency || '—'} 并发`;
  return <Badge tone={form.state === 'off' ? 'warn' : 'muted'}>{label}</Badge>;
}

/**
 * 频道行的配置型徽章（`p02-groups.png`）。
 *
 * - `tvg-id` / `Logo`：频道带了 EPG 标识或台标；
 * - `常驻`：`livestream = true`，服务启动了本地回看缓冲；
 * - `全轨道`：DASH 频道未选子集轨道（`selected_tracks` 为空）。
 *
 * 运行型徽章来自 `/panel/api/health` 的 `entries`：
 * - 绿点：最近一次探测成功；
 * - `异常`：最近一次探测失败。
 */
function ChannelBadges({
  group,
  channel,
  health,
}: {
  group: Group;
  channel: Channel;
  health?: HealthEntry;
}) {
  const tvgId = channel.tvg_id ?? channel.tvg_name;
  const logo = channel.logo ?? channel.tvg_logo;
  const tracks = channel.selected_tracks ?? [];
  return (
    <>
      {health?.ok === true && <Badge tone="ok">●</Badge>}
      {health?.ok === false && <Badge tone="warn">异常</Badge>}
      {typeof tvgId === 'string' && tvgId !== '' && <Badge tone="muted">tvg-id</Badge>}
      {typeof logo === 'string' && logo !== '' && <Badge tone="muted">Logo</Badge>}
      {channel.livestream === true && <Badge tone="info">常驻</Badge>}
      {group.mode === 'dash' && tracks.length === 0 && <Badge tone="muted">全轨道</Badge>}
    </>
  );
}

export function GroupsPage() {
  const { node, ok, err } = useToast();
  const [groups, setGroups] = useState<Group[]>([]);
  const [filter, setFilter] = useState('');
  const [groupForm, setGroupForm] = useState<{ form: GroupForm; isNew: boolean } | null>(null);
  const [channelForm, setChannelForm] = useState<{
    gid: string;
    form: ChannelForm;
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

  const saveGroup = async () => {
    if (!groupForm) return;
    const { form, isNew } = groupForm;
    const prefetch = prefetchPayload(form.prefetch);
    try {
      if (isNew) {
        await api('POST', '/groups', { ...form, prefetch });
      } else {
        await api('PUT', `/groups/${form.id}`, {
          name: form.name,
          mode: form.mode,
          access: form.access,
          prefetch,
        });
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
    const { gid, form, isNew } = channelForm;
    const prefetch = prefetchPayload(form.prefetch);
    try {
      if (isNew) {
        await api('POST', `/groups/${gid}/channels`, { ...form, prefetch });
      } else {
        await api('PUT', `/groups/${gid}/channels/${form.id}`, {
          name: form.name,
          url: form.url,
          prefetch,
        });
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

  const copy = async (text: string, label: string) => {
    const done = await copyText(text);
    if (done) ok(`${label}已复制`);
    else err('复制失败，请手动选取');
  };

  /**
   * 重排后整份回写（A9）。
   *
   * 目标没有单独的排序端点：它在前端改数组顺序后 `PUT /panel/api/config` 全量保存，
   * 本实现与之一致。播放列表按 `[[groups]]` / `[[groups.channels]]` 的数组顺序输出
   * （`playlist::render_total`），所以顺序即订阅里的顺序。
   */
  const reorder = async (mutate: (groups: Group[]) => boolean) => {
    try {
      const config = await api<{ groups?: Group[] } & Record<string, unknown>>('GET', '/config');
      const groups = config.groups ?? [];
      if (!mutate(groups)) return;
      await api('PUT', '/config', { ...config, groups });
      ok('顺序已保存');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const moveGroup = (index: number, delta: number) =>
    reorder((groups) => {
      const target = index + delta;
      if (target < 0 || target >= groups.length) return false;
      const [moved] = groups.splice(index, 1);
      groups.splice(target, 0, moved);
      return true;
    });

  const moveChannel = (gid: string, index: number, delta: number) =>
    reorder((groups) => {
      const channels = groups.find((item) => item.id === gid)?.channels;
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

  const keyword = filter.trim().toLowerCase();
  const visible = keyword
    ? groups.filter((group) => {
        if (`${group.id} ${group.name}`.toLowerCase().includes(keyword)) return true;
        return (group.channels ?? []).some((channel) =>
          `${channel.id} ${channel.name}`.toLowerCase().includes(keyword),
        );
      })
    : groups;

  const totalChannels = groups.reduce((sum, group) => sum + (group.channels?.length ?? 0), 0);

  return (
    <>
      <div className="card">
        <div className="row between">
          <div>
            <h2>分组管理</h2>
            <span className="muted">
              共 {groups.length} 个分组、{totalChannels} 个频道
            </span>
          </div>
          <div className="row">
            <input
              className="search"
              placeholder="搜索分组或频道"
              value={filter}
              onChange={(event) => setFilter(event.target.value)}
            />
            <button className="ghost" onClick={() => void load()}>
              刷新
            </button>
            <button
              className="ghost"
              onClick={() => {
                setImporting({ kind: 'm3u', force: false });
                setImportText('');
              }}
            >
              M3U 导入
            </button>
            <button
              className="ghost"
              onClick={() => {
                setImporting({ kind: 'json', force: false });
                setImportText('');
              }}
            >
              JSON 导入
            </button>
            <button
              className="ghost"
              onClick={() => {
                setImporting({ kind: 'json', force: true });
                setImportText('');
              }}
            >
              JSON 覆盖
            </button>
            <button className="ghost" onClick={() => void exportAll()}>
              导出全部
            </button>
            <button
              onClick={() =>
                setGroupForm({
                  form: {
                    id: '',
                    name: '',
                    mode: 'proxy',
                    access: 'public',
                    prefetch: emptyPrefetch(),
                  },
                  isNew: true,
                })
              }
            >
              新建分组
            </button>
          </div>
        </div>
      </div>

      {visible.length === 0 && <div className="card muted">没有匹配的分组。</div>}

      {visible.map((group) => {
        const channels = group.channels ?? [];
        const listUrl = `${location.origin}/${group.id}/playlist.m3u`;
        // 排序按 `groups` 的绝对下标；搜索过滤时也不会错位。
        const groupIndex = groups.indexOf(group);
        return (
          <div className="card" key={group.id}>
            <div className="row between">
              <div className="row">
                <button
                  className="icon-btn move"
                  disabled={groupIndex <= 0}
                  title="上移分组"
                  onClick={() => void moveGroup(groupIndex, -1)}
                >
                  ▲
                </button>
                <button
                  className="icon-btn move"
                  disabled={groupIndex < 0 || groupIndex >= groups.length - 1}
                  title="下移分组"
                  onClick={() => void moveGroup(groupIndex, 1)}
                >
                  ▼
                </button>
                <strong>{group.name || group.id}</strong>
                <span className="muted mono">{group.id}</span>
                <Badge tone="info">{group.mode}</Badge>
                <Badge tone={ACCESS_TONE[group.access] ?? 'muted'}>{group.access}</Badge>
                <PrefetchBadge raw={group.prefetch} />
                <span className="muted">{channels.length} 个频道</span>
              </div>
              <div className="row">
                <button className="ghost" onClick={() => void copy(listUrl, '订阅地址')}>
                  复制订阅
                </button>
                <a className="btn ghost" href={listUrl} target="_blank" rel="noreferrer">
                  打开订阅
                </a>
                <button className="ghost" onClick={() => void exportGroup(group.id)}>
                  导出
                </button>
                <button
                  className="ghost"
                  onClick={() =>
                    setGroupForm({
                      form: {
                        id: group.id,
                        name: group.name,
                        mode: group.mode,
                        access: group.access,
                        prefetch: prefetchForm(group.prefetch),
                      },
                      isNew: false,
                    })
                  }
                >
                  编辑
                </button>
                <button
                  className="ghost danger-text"
                  onClick={() =>
                    setRemoving({ kind: 'group', gid: group.id, label: group.name || group.id })
                  }
                >
                  删除
                </button>
              </div>
            </div>

            <table className="table">
              <thead>
                <tr>
                  <th>频道</th>
                  <th>上游地址</th>
                  <th>播放地址</th>
                  <th className="right">操作</th>
                </tr>
              </thead>
              <tbody>
                {channels.map((channel, channelIndex) => (
                  <tr key={channel.id}>
                    <td>
                      {channel.name || channel.id}
                      <div className="muted mono">{channel.id}</div>
                      <PrefetchBadge raw={channel.prefetch} />
                      <ChannelBadges
                        group={group}
                        channel={channel}
                        health={health[`${group.id}/${channel.id}`]}
                      />
                    </td>
                    <td className="mono ellipsis">{channel.url}</td>
                    <td className="mono ellipsis">
                      {group.mode === 'passthrough' ? (
                        <>
                          {channel.url} <Badge tone="muted">直通</Badge>
                        </>
                      ) : (
                        entryUrl(group, channel)
                      )}
                    </td>
                    <td className="right nowrap">
                      <button
                        className="icon-btn move"
                        disabled={channelIndex === 0}
                        title="上移频道"
                        onClick={() => void moveChannel(group.id, channelIndex, -1)}
                      >
                        ▲
                      </button>
                      <button
                        className="icon-btn move"
                        disabled={channelIndex >= channels.length - 1}
                        title="下移频道"
                        onClick={() => void moveChannel(group.id, channelIndex, 1)}
                      >
                        ▼
                      </button>
                      <button
                        className="ghost"
                        onClick={() =>
                          void copy(
                            group.mode === 'passthrough'
                              ? channel.url
                              : entryUrl(group, channel),
                            '播放地址',
                          )
                        }
                      >
                        复制
                      </button>
                      {group.mode === 'dash' && (
                        <button
                          className="ghost"
                          onClick={() => void showTracks(group.id, channel.id)}
                        >
                          轨道
                        </button>
                      )}
                      <button
                        className="ghost"
                        onClick={() =>
                          setChannelForm({
                            gid: group.id,
                            form: {
                              id: channel.id,
                              name: channel.name,
                              url: channel.url,
                              prefetch: prefetchForm(channel.prefetch),
                            },
                            isNew: false,
                          })
                        }
                      >
                        编辑
                      </button>
                      <button
                        className="ghost danger-text"
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
                    </td>
                  </tr>
                ))}
                {channels.length === 0 && (
                  <tr>
                    <td colSpan={4} className="muted">
                      暂无频道
                    </td>
                  </tr>
                )}
              </tbody>
            </table>

            <div className="row" style={{ marginTop: 10 }}>
              <button
                className="ghost"
                onClick={() =>
                  setChannelForm({
                    gid: group.id,
                    form: { id: '', name: '', url: '', prefetch: emptyPrefetch() },
                    isNew: true,
                  })
                }
              >
                + 新建频道
              </button>
            </div>
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
          <Field label="分组 ID">
            <input
              value={groupForm.form.id}
              disabled={!groupForm.isNew}
              onChange={(event) =>
                setGroupForm({ ...groupForm, form: { ...groupForm.form, id: event.target.value } })
              }
            />
          </Field>
          <Field label="显示名称">
            <input
              value={groupForm.form.name}
              onChange={(event) =>
                setGroupForm({ ...groupForm, form: { ...groupForm.form, name: event.target.value } })
              }
            />
          </Field>
          <Field label="播放模式">
            <Select
              value={groupForm.form.mode}
              options={MODES}
              onChange={(value) =>
                setGroupForm({ ...groupForm, form: { ...groupForm.form, mode: value } })
              }
            />
          </Field>
          <Field label="访问级别">
            <Select
              value={groupForm.form.access}
              options={ACCESS}
              onChange={(value) =>
                setGroupForm({ ...groupForm, form: { ...groupForm.form, access: value } })
              }
            />
          </Field>
          <PrefetchFields
            form={groupForm.form.prefetch}
            onChange={(prefetch) =>
              setGroupForm({ ...groupForm, form: { ...groupForm.form, prefetch } })
            }
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
          <Field label="频道 ID">
            <input
              value={channelForm.form.id}
              disabled={!channelForm.isNew}
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
              onChange={(event) =>
                setChannelForm({
                  ...channelForm,
                  form: { ...channelForm.form, name: event.target.value },
                })
              }
            />
          </Field>
          <Field label="上游地址">
            <input
              value={channelForm.form.url}
              onChange={(event) =>
                setChannelForm({
                  ...channelForm,
                  form: { ...channelForm.form, url: event.target.value },
                })
              }
            />
          </Field>
          <PrefetchFields
            form={channelForm.form.prefetch}
            onChange={(prefetch) =>
              setChannelForm({ ...channelForm, form: { ...channelForm.form, prefetch } })
            }
          />
        </Modal>
      )}

      {importing && (
        <Modal
          title={
            importing.kind === 'm3u'
              ? 'M3U 导入'
              : importing.force
                ? 'JSON 覆盖导入'
                : 'JSON 导入'
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
            <div className="field">
              <label>同名分组处理</label>
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
            </div>
          ) : (
            <p className="muted">
              {importing.force
                ? '按分组 ID 覆盖：已存在的分组整体替换，其余追加。'
                : '按分组 ID 追加：已存在的分组跳过，其余新增。'}
            </p>
          )}
          <div className="field">
            <label>{importing.kind === 'm3u' ? 'M3U 文本' : '分组 JSON 数组'}</label>
            <textarea
              rows={12}
              placeholder={
                importing.kind === 'm3u'
                  ? '#EXTM3U\n#EXTINF:-1 tvg-id="cctv1" group-title="央视",CCTV-1\nhttp://…'
                  : '[{ "id": "cctv", "name": "央视", "mode": "proxy", "channels": [] }]'
              }
              value={importText}
              onChange={(event) => setImportText(event.target.value)}
            />
          </div>
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

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <div className="field">
      <label>{label}</label>
      {children}
    </div>
  );
}

/** 预取覆盖：三态选择 + 数量/并发（留空表示不覆盖）。 */
function PrefetchFields({
  form,
  onChange,
}: {
  form: PrefetchForm;
  onChange: (next: PrefetchForm) => void;
}) {
  const numeric = form.state === 'inherit' || form.state === 'on' || form.state === 'off';
  return (
    <>
      <Field label="预取覆盖">
        <select
          value={form.state}
          onChange={(event) => onChange({ ...form, state: event.target.value as PrefetchForm['state'] })}
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
          <Field label="单次上限（留空不覆盖）">
            <input
              type="number"
              min={0}
              value={form.limit}
              placeholder="0 = 不限"
              onChange={(event) => onChange({ ...form, limit: event.target.value })}
            />
          </Field>
          <Field label="并发数（留空不覆盖）">
            <input
              type="number"
              min={1}
              value={form.concurrency}
              placeholder="1-32"
              onChange={(event) => onChange({ ...form, concurrency: event.target.value })}
            />
          </Field>
        </div>
      )}
    </>
  );
}

function Select({
  value,
  options,
  onChange,
}: {
  value: string;
  options: string[];
  onChange: (value: string) => void;
}) {
  return (
    <select value={value} onChange={(event) => onChange(event.target.value)}>
      {options.map((option) => (
        <option key={option} value={option}>
          {option}
        </option>
      ))}
    </select>
  );
}
