/**
 * 用户与设备分区。
 *
 * 功能对齐 mpd-hls 的 `/users`（`docs/design/09-users-and-devices.md` §4）。
 * 接口：`/panel/api/users`、`/panel/api/users/:id/tokens`。
 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { Icon } from './icons';
import { Badge, Confirm, Field, Modal, PageHead, Select, Switch, useToast } from './ui';

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
type OptionGroup = {
  id: string;
  name: string;
  origin?: string;
  source_id?: string;
  source_name?: string;
  channels: OptionChannel[];
};

type UserForm = {
  username: string;
  password: string;
  role: Role;
  all_groups: boolean;
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
    all_groups: false,
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
    // 空列表表示全部分组。
    all_groups: user.allowed_group_ids.length === 0,
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

/** 自定义凭证的本地校验。空串表示随机生成。返回空串表示通过。 */
function credentialProblem(value: string): string {
  const trimmed = value.trim();
  if (trimmed === '') return '';
  if (!/^[A-Za-z0-9_-]{8,64}$/.test(trimmed)) {
    return '自定义凭证须为 8–64 位字母、数字、下划线或连字符。';
  }
  return '';
}

/** 凭证有效期文本。 */
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
  const [tokenCredential, setTokenCredential] = useState('');
  const [busy, setBusy] = useState(false);
  const [passwordFor, setPasswordFor] = useState<User | null>(null);
  const [newPassword, setNewPassword] = useState('');
  const [confirmRemove, setConfirmRemove] = useState<User | null>(null);
  const [confirmRevoke, setConfirmRevoke] = useState<AccessToken | null>(null);

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
    if (form.role === 'user' && !form.all_groups && form.allowed_group_ids.length === 0) {
      err('普通用户至少需要一个可见分组，或开启「可见全部分组」。');
      return;
    }
    setBusy(true);
    try {
      // 空列表表示全部分组。管理员不受分组限制，统一发空列表。
      const payload = {
        username: form.username,
        role: form.role,
        allowed_group_ids:
          form.role === 'admin' || form.all_groups ? [] : form.allowed_group_ids,
        channel_filter_enabled: form.channel_filter_enabled,
        allowed_channel_ids: form.allowed_channel_ids,
        web_ui_access: form.web_ui_access,
      };
      if (id === null) {
        await api('POST', '/users', { ...payload, password: form.password });
        ok('用户已创建');
      } else {
        await api('PUT', `/users/${id}`, payload);
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

  const resetPassword = async () => {
    if (!passwordFor || newPassword === '') return;
    setBusy(true);
    try {
      await api('PUT', `/users/${passwordFor.id}/password`, { password: newPassword });
      ok('密码已重置，该用户需要重新登录');
      setPasswordFor(null);
      setNewPassword('');
    } catch (error) {
      err(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const removeUser = async (user: User) => {
    setBusy(true);
    try {
      await api('DELETE', `/users/${user.id}`);
      ok('用户已删除');
      await load();
    } catch (error) {
      err(errorText(error));
    } finally {
      setBusy(false);
    }
  };

  const openTokens = async (user: User) => {
    setTokenFor(user);
    setTokenLabel('');
    setTokenTtl(null);
    setTokenCredential('');
    await loadTokens(user.id);
  };

  const createToken = async () => {
    if (!tokenFor) return;
    const custom = tokenCredential.trim();
    const problem = credentialProblem(custom);
    if (problem !== '') {
      err(problem);
      return;
    }
    try {
      await api('POST', `/users/${tokenFor.id}/tokens`, {
        label: tokenLabel.trim() === '' ? null : tokenLabel.trim(),
        ttl_secs: tokenTtl,
        token: custom === '' ? null : custom,
      });
      setTokenLabel('');
      setTokenTtl(null);
      setTokenCredential('');
      await loadTokens(tokenFor.id);
      ok('设备已添加');
    } catch (error) {
      err(errorText(error));
    }
  };

  const revokeToken = async (token: AccessToken) => {
    if (!tokenFor) return;
    try {
      await api('DELETE', `/users/${tokenFor.id}/tokens/${token.token}`);
      await loadTokens(tokenFor.id);
      ok('设备已撤销');
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
  const visibleChannels = form?.all_groups
    ? options
    : options.filter((group) => form?.allowed_group_ids.includes(group.id));
  const credentialIssue = credentialProblem(tokenCredential);

  return (
    <>
      {node}
      <PageHead icon="users" title="用户与设备" sub="管理面板账号与每个用户能看的分组">
        <button
          title="创建面板账号并设置可见范围"
          onClick={() => setEditing({ form: emptyForm(), id: null })}
        >
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
              <th>面板登录</th>
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
                  {user.role === 'admin' ? (
                    '全部'
                  ) : user.allowed_group_ids.length === 0 ? (
                    <span title="包含以后新增的分组">全部</span>
                  ) : (
                    user.allowed_group_ids.map(groupName).join('、')
                  )}
                  {user.channel_filter_enabled && (
                    <Badge tone="info">
                      {user.allowed_channel_ids.length} 个频道
                    </Badge>
                  )}
                </td>
                <td>{user.web_ui_access || user.role === 'admin' ? '允许' : '禁止'}</td>
                <td className="muted">{formatDate(user.created_at_ms)}</td>
                <td className="right">
                  <button
                    className="chip"
                    title="管理这个用户的设备凭证"
                    onClick={() => void openTokens(user)}
                  >
                    设备
                  </button>
                  <button
                    className="chip"
                    title="修改用户名、角色与可见范围"
                    onClick={() => setEditing({ form: formOf(user), id: user.id })}
                  >
                    编辑
                  </button>
                  <button
                    className="chip"
                    title="设置新密码，旧会话会失效"
                    onClick={() => {
                      setPasswordFor(user);
                      setNewPassword('');
                    }}
                  >
                    重置密码
                  </button>
                  <button
                    className="chip danger-text"
                    title="删除用户，同时撤销其全部设备"
                    onClick={() => setConfirmRemove(user)}
                  >
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
              ? '用户名与密码均为必填；普通用户至少需要一个可见分组，或开启「可见全部分组」。'
              : '修改用户名、角色或可见分组。密码请通过「重置密码」按钮修改。'}
          </p>
          <div className="fields">
            <Field
              label="用户名"
              tip="登录面板时使用。同一实例内不能重名。"
            >
              <input
                value={form.username}
                autoComplete="off"
                onChange={(event) =>
                  setEditing({ ...editing, form: { ...form, username: event.target.value } })
                }
              />
            </Field>
            {isNew && (
              <Field
                label="密码"
                tip="初始登录口令。用户登录后可以自行修改。"
              >
                <input
                  type="password"
                  autoComplete="new-password"
                  value={form.password}
                  onChange={(event) =>
                    setEditing({ ...editing, form: { ...form, password: event.target.value } })
                  }
                />
              </Field>
            )}
            <Field
              label="角色"
              tip="管理员可管理所有分区。普通用户只能查看授权频道。"
            >
              <Select
                ariaLabel="角色"
                value={form.role}
                options={[
                  { value: 'admin', label: '管理员', desc: '可以管理所有分区。' },
                  { value: 'user', label: '普通用户', desc: '只能查看授权的频道。' },
                ]}
                onChange={(next) =>
                  setEditing({ ...editing, form: { ...form, role: next as Role } })
                }
                compact
              />
            </Field>
          </div>

          {form.role === 'user' && (
            <>
              <div className="row between">
                <div>
                  <div className="section-title">可见分组</div>
                  <p className="muted">
                    普通用户只能看到选中的分组。管理员不受分组限制。
                  </p>
                </div>
                <Switch
                  label="可见全部分组"
                  checked={form.all_groups}
                  onChange={(next) =>
                    setEditing({
                      ...editing,
                      form: { ...form, all_groups: next },
                    })
                  }
                />
              </div>
              {form.all_groups ? (
                <p className="muted">
                  可见全部分组，包括以后新增的分组。管理员不受分组限制。
                </p>
              ) : (
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
                      {group.source_id ? (
                        <Badge tone="info">来自订阅</Badge>
                      ) : (
                        <Badge tone="muted">本地</Badge>
                      )}
                    </label>
                  ))}
                  {options.length === 0 && <p className="muted">还没有分组。</p>}
                </div>
              )}

              <div className="row between">
                <div>
                  <div className="section-title">只开放指定频道</div>
                  <p className="muted">
                    开启后，频道列表、节目单、订阅和已有的播放地址都只允许勾选的频道。
                  </p>
                </div>
                <Switch
                  label="只开放指定频道"
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
              <div className="section-title">允许登录面板</div>
              <p className="muted">
                登录后只能查看授权的频道和节目单，并复制播放地址。
              </p>
            </div>
            <Switch
              label="允许登录面板"
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
            每个设备有一个独立播放凭证。播放地址形如{' '}
            <code className="mono">/sub/&lt;凭证&gt;/playlist.m3u</code>。
          </p>
          <div className="device-add">
            <div className="fields">
              <Field label="设备名称" hint="可选。只用于区分设备。">
                <input
                  aria-label="设备名称"
                  placeholder="例如 客厅电视"
                  value={tokenLabel}
                  onChange={(event) => setTokenLabel(event.target.value)}
                />
              </Field>
              <Field label="有效期" hint="到期后订阅地址自动失效。">
                <Select
                  ariaLabel="有效期"
                  value={tokenTtl === null ? 'forever' : String(tokenTtl)}
                  options={TTL_CHOICES.map((item) => ({
                    value: item.secs === null ? 'forever' : String(item.secs),
                    label: item.label,
                  }))}
                  onChange={(next) => setTokenTtl(next === 'forever' ? null : Number(next))}
                />
              </Field>
            </div>
            <Field
              label="自定义凭证"
              hint={
                credentialIssue === ''
                  ? '可选。留空时随机生成 12 位。只能使用字母、数字、下划线和连字符，长度 8–64 位。'
                  : undefined
              }
            >
              <input
                aria-label="自定义凭证"
                placeholder="留空则自动生成"
                value={tokenCredential}
                onChange={(event) => setTokenCredential(event.target.value)}
                spellCheck={false}
                autoComplete="off"
                aria-invalid={credentialIssue !== ''}
              />
            </Field>
            {credentialIssue !== '' && (
              <div className="hint danger-text">{credentialIssue}</div>
            )}
            <div className="row between device-add-actions">
              <span className="muted">每个设备一个独立凭证。建成后不可修改。</span>
              <button
                title="为这个用户生成一个新的设备凭证"
                onClick={() => void createToken()}
                disabled={credentialIssue !== ''}
              >
                添加设备
              </button>
            </div>
          </div>

          <table className="table device-table">
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
                  <td>
                    <span className="device-name" title={token.label ?? '未命名'}>
                      {token.label || <span className="muted">未命名</span>}
                      {token.purpose === 'web_ui_links' && (
                        <span className="muted"> · 面板</span>
                      )}
                    </span>
                  </td>
                  <td className="mono ellipsis">{token.playlist_url}</td>
                  <td className="muted">{ttlText(token)}</td>
                  <td className="right nowrap">
                    <button
                      className="chip"
                      title="复制这个设备的完整订阅地址"
                      onClick={() => void copy(token.playlist_url, '订阅地址')}
                    >
                      复制
                    </button>
                    <button
                      className="chip danger-text"
                      title="让这个设备的播放地址立即失效。要换凭证，请撤销后重新添加。"
                      onClick={() => setConfirmRevoke(token)}
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

      {passwordFor ? (
        <Modal
          title={`重置密码 · ${passwordFor.username}`}
          onClose={() => setPasswordFor(null)}
          footer={
            <>
              <button className="ghost" onClick={() => setPasswordFor(null)}>
                取消
              </button>
              <button
                disabled={busy || newPassword === ''}
                onClick={() => void resetPassword()}
              >
                {busy ? '保存中…' : '重置密码'}
              </button>
            </>
          }
        >
          <Field
            label="新密码"
            tip="保存后该用户的旧会话失效，需要用新密码重新登录。"
          >
            <input
              type="password"
              autoComplete="new-password"
              value={newPassword}
              onChange={(event) => setNewPassword(event.target.value)}
            />
          </Field>
        </Modal>
      ) : null}

      {confirmRemove ? (
        <Confirm
          title="删除用户？"
          text={`用户「${confirmRemove.username}」及其全部设备会一起删除。这个操作不能撤销。`}
          confirmText="删除用户"
          busy={busy}
          onCancel={() => setConfirmRemove(null)}
          onConfirm={() => {
            const user = confirmRemove;
            setConfirmRemove(null);
            void removeUser(user);
          }}
        />
      ) : null}

      {confirmRevoke ? (
        <Confirm
          title="撤销这个设备？"
          text="使用这个凭证的播放地址会立即失效。其他设备不受影响。"
          confirmText="撤销设备"
          onCancel={() => setConfirmRevoke(null)}
          onConfirm={() => {
            const token = confirmRevoke;
            setConfirmRevoke(null);
            void revokeToken(token);
          }}
        />
      ) : null}
    </>
  );
}
