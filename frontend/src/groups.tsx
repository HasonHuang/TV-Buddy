/** 分组与频道管理（M1）。 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { Badge, Confirm, Modal, useToast } from './ui';

export type Channel = {
  id: string;
  name: string;
  url: string;
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

  const load = useCallback(async () => {
    try {
      setGroups(await api<Group[]>('GET', '/groups/export'));
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
        return (
          <div className="card" key={group.id}>
            <div className="row between">
              <div className="row">
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
                {channels.map((channel) => (
                  <tr key={channel.id}>
                    <td>
                      {channel.name || channel.id}
                      <div className="muted mono">{channel.id}</div>
                      <PrefetchBadge raw={channel.prefetch} />
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
