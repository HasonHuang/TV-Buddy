/**
 * 用户与设备分区。
 *
 * 功能对齐 mpd-hls 的 `/users`（`docs/design/09-users-and-devices.md` §4）。
 * 接口：`/panel/api/users`、`/panel/api/users/:id/tokens`。
 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { Icon } from './icons';
import { Badge, Field, Modal, PageHead, Switch, useToast } from './ui';

type Role = 'admin' | 'user';

type User = {
  id: string;
  username: string;
  role: Role;
  allowed_group_ids: string[];
  channel_filter_enabled: boolean;
  allowed_channel_ids: string[];
  web_ui_access: boolean;
  created_at_ms: number;
  updated_at_ms: number;
};

type AccessToken = {
  token: string;
  user_id: string;
  purpose: 'device' | 'web_ui_links';
  label: string | null;
  created_at_ms: number;
  expires_at_ms: number | null;
  last_seen_at_ms: number | null;
  playlist_url: string;
  epg_url: string;
};

type OptionChannel = { id: string; name: string; key: string };
type OptionGroup = { id: string; name: string; channels: OptionChannel[] };

type UserForm = {
  username: string;
  password: string;
  role: Role;
  allowed_group_ids: string[];
  channel_filter_enabled: boolean;
  allowed_channel_ids: string[];
  web_ui_access: boolean;
};

const TTL_CHOICES: { label: string; secs: number | null }[] = [
  { label: '1 小时', secs: 3600 },
  { label: '6 小时', secs: 21600 },
  { label: '12 小时', secs: 43200 },
  { label: '7 天', secs: 604800 },
  { label: '30 天', secs: 2592000 },
  { label: '90 天', secs: 7776000 },
  { label: '1 年', secs: 31536000 },
  { label: '2 年', secs: 63072000 },
  { label: '3 年', secs: 94608000 },
  { label: '永不过期', secs: null },
];

function emptyForm(): UserForm {
  return {
    username: '',
    password: '',
    role: 'user',
    allowed_group_ids: [],
    channel_filter_enabled: false,
    allowed_channel_ids: [],
    web_ui_access: false,
  };
}

function formOf(user: User): UserForm {
  return {
    username: user.username,
    password: '',
    role: user.role,
    allowed_group_ids: [...user.allowed_group_ids],
    channel_filter_enabled: user.channel_filter_enabled,
    allowed_channel_ids: [...user.allowed_channel_ids],
    web_ui_access: user.web_ui_access,
  };
}

/** 毫秒时间戳转本地日期。 */
function formatDate(ms: number): string {
  if (!ms) return '—';
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(date.getHours())}:${pad(date.getMinutes())}`;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 令牌有效期文本。 */
function ttlText(token: AccessToken): string {
  if (token.expires_at_ms === null) return '永不过期';
  const left = token.expires_at_ms - Date.now();
  if (left <= 0) return '已过期';
  const days = Math.floor(left / 86_400_000);
  if (days >= 1) return `${days} 天后到期`;
  const hours = Math.floor(left / 3_600_000);
  if (hours >= 1) return `${hours} 小时后到期`;
  return `${Math.max(1, Math.floor(left / 60_000))} 分钟后到期`;
}

export function UsersPage() {
  const { node, ok, err } = useToast();
  const [users, setUsers] = useState<User[]>([]);
  const [options, setOptions] = useState<OptionGroup[]>([]);
  const [editing, setEditing] = useState<{ form: UserForm; id: string | null } | null>(null);
  const [tokenFor, setTokenFor] = useState<User | null>(null);
  const [tokens, setTokens] = useState<AccessToken[]>([]);
  const [tokenLabel, setTokenLabel] = useState('');
  const [tokenTtl, setTokenTtl] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [body, opts] = await Promise.all([
        api<{ users?: User[] }>('GET', '/users'),
        api<{ groups?: OptionGroup[] }>('GET', '/users/channel-options'),
      ]);
      setUsers(body.users ?? []);
      setOptions(opts.groups ?? []);
    } catch (error) {
      err(errorText(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const loadTokens = useCallback(async (userId: string) => {
    try {
      const body = await api<{ tokens?: AccessToken[] }>('GET', `/users/${userId}/tokens`);
      setTokens(body.tokens ?? []);
    } catch (error) {
      err(errorText(error));
    }
  }, [err]);

  const saveUser = async () => {
    if (!editing) return;
    const { form, id } = editing;
    setBusy(true);
    try {
      if (id === null) {
        await api('POST', '/users', form);
        ok('用户已创建');
      } else {
        await api('PUT', `/users/${id}`, {
          username: form.username,
          role: form.role,
          allowed_group_ids: form.allowed_group_ids,
          channel_filter_enabled: form.channel_filter_enabled,
          allowed_channel_ids: form.allowed_channel_ids,
          web_ui_access: form.web_ui_access,
        });
        ok('用户已保存');
      }
      setEditing(null);
      await load();
    } catch (error) {
      err(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const resetPassword = async (user: User) => {
    const password = window.prompt(`给 ${user.username} 设置新密码`, '');
    if (!password) return;
    try {
      await api('PUT', `/users/${user.id}/password`, { password });
      ok('密码已重置，该用户需要重新登录');
    } catch (error) {
      err(errorText(error));
    }
  };

  const removeUser = async (user: User) => {
    if (!window.confirm(`删除用户 ${user.username}？其全部设备令牌同时失效。`)) return;
    try {
      await api('DELETE', `/users/${user.id}`);
      ok('用户已删除');
      await load();
    } catch (error) {
      err(errorText(error));
    }
  };

  const openTokens = async (user: User) => {
    setTokenFor(user);
    setTokenLabel('');
    setTokenTtl(null);
    await loadTokens(user.id);
  };

  const createToken = async () => {
    if (!tokenFor) return;
    try {
      await api('POST', `/users/${tokenFor.id}/tokens`, {
        label: tokenLabel.trim() === '' ? null : tokenLabel.trim(),
        ttl_secs: tokenTtl,
      });
      setTokenLabel('');
      setTokenTtl(null);
      await loadTokens(tokenFor.id);
      ok('设备已添加');
    } catch (error) {
      err(errorText(error));
    }
  };

  const revokeToken = async (token: AccessToken) => {
    if (!tokenFor) return;
    if (!window.confirm('撤销这个设备令牌？用它生成的播放地址会立刻失效。')) return;
    try {
      await api('DELETE', `/users/${tokenFor.id}/tokens/${token.token}`);
      await loadTokens(tokenFor.id);
      ok('令牌已撤销');
    } catch (error) {
      err(errorText(error));
    }
  };

  const copy = async (value: string, label: string) => {
    const full = `${location.origin}${value}`;
    if (await copyText(full)) ok(`${label}已复制`);
    else err('复制失败，请手动选取');
  };

  const groupName = (gid: string) =>
    options.find((group) => group.id === gid)?.name || gid;

  /** 勾选或取消一个分组。取消时连带清掉该分组下的频道授权。 */
  const toggleGroup = (gid: string) => {
    setEditing((current) => {
      if (!current) return current;
      const has = current.form.allowed_group_ids.includes(gid);
      const groups = has
        ? current.form.allowed_group_ids.filter((id) => id !== gid)
        : [...current.form.allowed_group_ids, gid];
      const channels = has
        ? current.form.allowed_channel_ids.filter((key) => !key.startsWith(`${gid}/`))
        : current.form.allowed_channel_ids;
      return {
        ...current,
        form: { ...current.form, allowed_group_ids: groups, allowed_channel_ids: channels },
      };
    });
  };

  const toggleChannel = (key: string) => {
    setEditing((current) => {
      if (!current) return current;
      const has = current.form.allowed_channel_ids.includes(key);
      return {
        ...current,
        form: {
          ...current.form,
          allowed_channel_ids: has
            ? current.form.allowed_channel_ids.filter((item) => item !== key)
            : [...current.form.allowed_channel_ids, key],
        },
      };
    });
  };

  const form = editing?.form;
  const isNew = editing?.id === null;
  const visibleChannels = options.filter((group) =>
    form?.allowed_group_ids.includes(group.id),
  );

  return (
    <>
      {node}
      <PageHead icon="users" title="用户与设备" sub="管理员与普通观众账户，下发可见分组范围">
        <button onClick={() => setEditing({ form: emptyForm(), id: null })}>
          <Icon name="plus" size={14} />
          新建用户
        </button>
      </PageHead>

      <div className="card flush">
        <table className="table" style={{ marginTop: 0 }}>
          <thead>
            <tr>
              <th>用户名</th>
              <th>角色</th>
              <th>可见分组</th>
              <th>WebUI 登录</th>
              <th>创建时间</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {users.map((user) => (
              <tr key={user.id}>
                <td className="mono">{user.username}</td>
                <td>
                  {user.role === 'admin' ? (
                    <Badge tone="ok">管理员</Badge>
                  ) : (
                    <Badge tone="muted">普通用户</Badge>
                  )}
                </td>
                <td>
                  {user.role === 'admin' || user.allowed_group_ids.length === 0
                    ? '全部'
                    : user.allowed_group_ids.map(groupName).join('、')}
                  {user.channel_filter_enabled && (
                    <Badge tone="info">
                      {user.allowed_channel_ids.length} 个频道
                    </Badge>
                  )}
                </td>
                <td>{user.web_ui_access || user.role === 'admin' ? '允许' : '禁止'}</td>
                <td className="muted">{formatDate(user.created_at_ms)}</td>
                <td className="right">
                  <button className="chip" onClick={() => void openTokens(user)}>
                    设备
                  </button>
                  <button
                    className="chip"
                    onClick={() => setEditing({ form: formOf(user), id: user.id })}
                  >
                    编辑
                  </button>
                  <button className="chip" onClick={() => void resetPassword(user)}>
                    重置密码
                  </button>
                  <button className="chip danger" onClick={() => void removeUser(user)}>
                    删除
                  </button>
                </td>
              </tr>
            ))}
            {users.length === 0 && (
              <tr>
                <td colSpan={6} className="muted">
                  还没有任何用户。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      {editing && form && (
        <Modal
          title={isNew ? '新建用户' : `编辑用户 ${form.username}`}
          onClose={() => setEditing(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setEditing(null)}>
                取消
              </button>
              <button disabled={busy} onClick={() => void saveUser()}>
                {busy ? '保存中…' : '保存'}
              </button>
            </>
          }
        >
          <p className="muted">
            {isNew
              ? '用户名与密码均为必填；普通用户至少需要一个可见分组。'
              : '修改用户名、角色或可见分组。密码请通过「重置密码」按钮修改。'}
          </p>
          <div className="fields">
            <Field label="用户名">
              <input
                value={form.username}
                onChange={(event) =>
                  setEditing({ ...editing, form: { ...form, username: event.target.value } })
                }
              />
            </Field>
            {isNew && (
              <Field label="密码">
                <input
                  type="password"
                  value={form.password}
                  onChange={(event) =>
                    setEditing({ ...editing, form: { ...form, password: event.target.value } })
                  }
                />
              </Field>
            )}
            <Field label="角色">
              <select
                value={form.role}
                onChange={(event) =>
                  setEditing({
                    ...editing,
                    form: { ...form, role: event.target.value as Role },
                  })
                }
              >
                <option value="admin">管理员</option>
                <option value="user">普通用户</option>
              </select>
            </Field>
          </div>

          {form.role === 'user' && (
            <>
              <Field label="可见分组" hint="普通用户至少选一个。管理员不受限制。">
                <div className="checkbox-list">
                  {options.map((group) => (
                    <label key={group.id} className="check">
                      <input
                        type="checkbox"
                        checked={form.allowed_group_ids.includes(group.id)}
                        onChange={() => toggleGroup(group.id)}
                      />
                      {group.name}
                      <span className="gid">{group.id}</span>
                    </label>
                  ))}
                  {options.length === 0 && <p className="muted">还没有分组。</p>}
                </div>
              </Field>

              <div className="row between">
                <div>
                  <div className="section-title">只开放指定频道</div>
                  <p className="muted">
                    开启后，频道列表、EPG、订阅与已有播放链接都只允许勾选的频道。
                  </p>
                </div>
                <Switch
                  checked={form.channel_filter_enabled}
                  onChange={(next) =>
                    setEditing({
                      ...editing,
                      form: { ...form, channel_filter_enabled: next },
                    })
                  }
                />
              </div>

              {form.channel_filter_enabled && (
                <div className="checkbox-list">
                  {visibleChannels.map((group) => (
                    <div key={group.id}>
                      <div className="muted">{group.name}</div>
                      {group.channels.map((channel) => (
                        <label key={channel.key} className="check">
                          <input
                            type="checkbox"
                            checked={form.allowed_channel_ids.includes(channel.key)}
                            onChange={() => toggleChannel(channel.key)}
                          />
                          {channel.name}
                        </label>
                      ))}
                      {group.channels.length === 0 && <p className="muted">该分组没有频道。</p>}
                    </div>
                  ))}
                  {visibleChannels.length === 0 && (
                    <p className="muted">先勾选上方可见分组，再选择频道。</p>
                  )}
                </div>
              )}
            </>
          )}

          <div className="row between">
            <div>
              <div className="section-title">允许登录 WebUI</div>
              <p className="muted">
                登录后只能查看授权频道、EPG，并复制安全播放链接。
              </p>
            </div>
            <Switch
              checked={form.web_ui_access}
              disabled={form.role === 'admin'}
              onChange={(next) =>
                setEditing({ ...editing, form: { ...form, web_ui_access: next } })
              }
            />
          </div>
        </Modal>
      )}

      {tokenFor && (
        <Modal
          title={`设备 · ${tokenFor.username}`}
          onClose={() => setTokenFor(null)}
          footer={
            <button className="ghost" onClick={() => setTokenFor(null)}>
              关闭
            </button>
          }
        >
          <p className="muted">
            每个设备一个令牌。播放地址形如{' '}
            <code className="mono">/sub/&lt;令牌&gt;/playlist.m3u</code>。
          </p>
          <div className="row form-row">
            <input
              placeholder="设备名称（可选）"
              value={tokenLabel}
              onChange={(event) => setTokenLabel(event.target.value)}
            />
            <select
              value={tokenTtl === null ? 'forever' : String(tokenTtl)}
              onChange={(event) =>
                setTokenTtl(event.target.value === 'forever' ? null : Number(event.target.value))
              }
            >
              {TTL_CHOICES.map((item) => (
                <option key={item.label} value={item.secs === null ? 'forever' : String(item.secs)}>
                  {item.label}
                </option>
              ))}
            </select>
            <button onClick={() => void createToken()}>添加设备</button>
          </div>

          <table className="table">
            <thead>
              <tr>
                <th>名称</th>
                <th>订阅地址</th>
                <th>有效期</th>
                <th className="right">操作</th>
              </tr>
            </thead>
            <tbody>
              {tokens.map((token) => (
                <tr key={token.token}>
                  <td>{token.label || <span className="muted">未命名</span>}</td>
                  <td className="mono ellipsis">{token.playlist_url}</td>
                  <td className="muted">{ttlText(token)}</td>
                  <td className="right">
                    <button className="chip" onClick={() => void copy(token.playlist_url, '订阅地址')}>
                      复制
                    </button>
                    <button
                      className="chip danger"
                      onClick={() => void revokeToken(token)}
                    >
                      撤销
                    </button>
                  </td>
                </tr>
              ))}
              {tokens.length === 0 && (
                <tr>
                  <td colSpan={4} className="muted">
                    还没有设备。
                  </td>
                </tr>
              )}
            </tbody>
          </table>
        </Modal>
      )}
    </>
  );
}
