/**
 * 「我的订阅」分区。
 *
 * 普通用户登录后只看到这一页。数据来自 `GET /api/panel/me`：
 * 订阅地址、只读设备列表与可见的分组/频道。
 * 管理员也看这一页，范围是自己的全部频道。
 *
 * 设备只读。新增、改名与撤销都只允许管理员操作。
 * 当前地址被封禁时，页首显示解封卡，不需要另找入口。
 */

import { useCallback, useEffect, useState } from 'react';

import {
  api,
  clearToken,
  copyText,
  getToken,
  UNAUTHORIZED_EVENT,
} from './api';
import { Icon } from './icons';
import { Badge, Empty, Loading, PageHead, Tip, useToast } from './ui';

type Channel = { id: string; name: string; url: string };
type Group = { id: string; name: string; channels: Channel[] };

/** 只读的设备视图。设备由管理员管理。 */
type Device = {
  token: string;
  label: string | null;
  created_at_ms: number;
  expires_at_ms: number | null;
  last_seen_at_ms: number | null;
  playlist_url: string;
  epg_url: string;
};

type Me = {
  user?: { username?: string; role?: string };
  token?: string;
  playlist_link?: string | null;
  epg_link?: string | null;
  groups?: Group[];
  devices?: Device[];
};

/** `GET /api/unban/whoami` 的结果。IP 管控关闭时只有 `enabled` 与 `ip`。 */
type IpState = {
  enabled?: boolean;
  banned?: boolean;
  ip?: string;
  remaining?: number;
};

/** 毫秒时间戳转本地时间。0 或空值显示占位符。 */
function formatTime(ms: number | null | undefined): string {
  if (!ms) return '—';
  const date = new Date(ms);
  const pad = (value: number) => String(value).padStart(2, '0');
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${pad(
    date.getHours(),
  )}:${pad(date.getMinutes())}`;
}

/** 设备状态。只读展示，不提供操作。 */
function deviceState(device: Device): {
  text: string;
  tone: string;
  detail: string;
} {
  if (device.expires_at_ms === null) {
    return { text: '永不过期', tone: 'ok', detail: '这台设备的凭证一直有效。' };
  }
  const until = formatTime(device.expires_at_ms);
  if (device.expires_at_ms <= Date.now()) {
    return { text: '已过期', tone: 'danger', detail: `有效期到 ${until}。` };
  }
  return { text: '有效', tone: 'info', detail: `有效期到 ${until}。` };
}

/** 读取当前地址的封禁状态。失败时返回 null，不影响页面主体。 */
async function fetchIpState(): Promise<IpState | null> {
  const token = getToken();
  if (!token) return null;
  try {
    const response = await fetch('/api/unban/whoami', {
      headers: { Authorization: `Bearer ${token}` },
    });
    if (!response.ok) return null;
    return (await response.json()) as IpState;
  } catch {
    return null;
  }
}

export function MySubsPage() {
  const { node, ok, err } = useToast();
  const [me, setMe] = useState<Me | null>(null);
  const [ipState, setIpState] = useState<IpState | null>(null);
  const [loading, setLoading] = useState(true);
  const [unbanBusy, setUnbanBusy] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      const [body, state] = await Promise.all([
        api<Me>('GET', '/me'),
        fetchIpState(),
      ]);
      setMe(body);
      setIpState(state);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const copy = async (value: string | null | undefined, label: string) => {
    if (!value) return;
    const done = await copyText(
      value.startsWith('/') ? `${location.origin}${value}` : value,
    );
    if (done) ok(`${label}已复制`);
    else err('复制失败，请手动选取');
  };

  /** 为当前地址自助解封。配额用完后由后端给出原因。 */
  const unbanSelf = async () => {
    setUnbanBusy(true);
    try {
      const response = await fetch('/api/unban', {
        method: 'POST',
        headers: { Authorization: `Bearer ${getToken()}` },
      });
      if (response.status === 401) {
        clearToken();
        window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
        return;
      }
      const body = (await response.json()) as {
        ok?: boolean;
        msg?: string;
        error?: string;
        remaining?: number;
      };
      if (body.ok) {
        ok(
          typeof body.remaining === 'number'
            ? `已解封。本月还可解封 ${body.remaining} 次。`
            : '已解封。',
        );
      } else {
        err(body.msg ?? body.error ?? '解封失败。');
      }
      await load();
    } catch {
      err('请求失败，请稍后重试。');
    } finally {
      setUnbanBusy(false);
    }
  };

  const groups = me?.groups ?? [];
  const devices = me?.devices ?? [];
  const total = groups.reduce((sum, group) => sum + group.channels.length, 0);
  const banned = ipState?.enabled === true && ipState.banned === true;

  const head = (
    <PageHead
      icon="subs"
      title="我的订阅"
      sub={
        loading && me === null
          ? '正在读取订阅信息'
          : `${devices.length} 台设备 · ${groups.length} 个分组 · ${total} 个频道`
      }
    >
      <button
        className="ghost"
        title="重新读取订阅地址、设备与授权频道"
        onClick={() => void load()}
        disabled={loading}
      >
        <Icon name="refresh" size={14} />
        {loading ? '刷新中…' : '刷新'}
      </button>
    </PageHead>
  );

  if (loading && me === null) {
    return (
      <>
        {head}
        <Loading />
        {node}
      </>
    );
  }

  return (
    <>
      {head}

      {banned ? (
        <div className="card ban-alert" role="alert">
          <span className="ban-alert-mark" aria-hidden="true">
            <Icon name="security" size={18} />
          </span>
          <div className="ban-alert-text">
            <strong>当前地址已被封禁</strong>
            <span className="muted">
              {ipState?.ip ? `IP ${ipState.ip} 暂时无法访问。` : '当前地址暂时无法访问。'}
              本月还可自助解封 {ipState?.remaining ?? 0} 次。
            </span>
          </div>
          <button
            className="danger"
            onClick={() => void unbanSelf()}
            disabled={unbanBusy}
            title="为当前地址提交一次解封请求"
          >
            {unbanBusy ? '提交中…' : '立即解封'}
          </button>
        </div>
      ) : null}

      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>
              订阅地址
              <Tip text="地址里带你的专属凭证。任何人拿到它都能播放你看得到的频道。" />
            </h2>
            <p className="muted">
              填进播放器的「订阅」。地址带专属凭证，不要外传。
            </p>
          </div>
        </div>
        <div className="row form-row">
          <input
            readOnly
            aria-label="订阅地址"
            title={me?.playlist_link ? `${location.origin}${me.playlist_link}` : ''}
            value={me?.playlist_link ? `${location.origin}${me.playlist_link}` : ''}
            className="mono"
          />
          <button
            className="chip"
            title="复制完整订阅地址"
            onClick={() => void copy(me?.playlist_link, '订阅地址')}
          >
            复制
          </button>
        </div>
        {me?.epg_link ? (
          <>
            <h2 className="sub-block">
              EPG 地址
              <Tip text="节目单只包含你看得到的频道。播放器支持时填这里。" />
            </h2>
            <p className="muted">只含你看得到的频道。播放器支持时填这里。</p>
            <div className="row form-row">
              <input
                readOnly
                aria-label="EPG 地址"
                title={me.epg_link ? `${location.origin}${me.epg_link}` : ''}
                value={me.epg_link ? `${location.origin}${me.epg_link}` : ''}
                className="mono"
              />
              <button
                className="chip"
                title="复制完整 EPG 地址"
                onClick={() => void copy(me.epg_link, 'EPG 地址')}
              >
                复制
              </button>
            </div>
          </>
        ) : null}
      </div>

      <div className="card">
        <div className="card-head">
          <div className="card-head-text">
            <h2>
              我的设备
              <Tip text="设备由管理员管理。这里只读，可查看与复制地址。" />
            </h2>
            <p className="muted">
              这里是只读视图。新增、改名或撤销设备，请联系管理员。
            </p>
          </div>
          <div className="card-actions">
            <Badge tone="muted">{devices.length} 台</Badge>
          </div>
        </div>

        {devices.length === 0 ? (
          <Empty text="还没有设备。请联系管理员添加。" icon="users" />
        ) : (
          <div className="device-grid">
            {devices.map((device) => {
              const state = deviceState(device);
              const name = device.label || '未命名设备';
              return (
                <article className="device-card" key={device.token}>
                  <div className="device-head">
                    <span className="device-icon" aria-hidden="true">
                      <Icon name="users" size={15} />
                    </span>
                    <span className="device-name" title={name}>
                      {name}
                    </span>
                    <Badge tone={state.tone}>{state.text}</Badge>
                  </div>

                  <div className="device-field">
                    <span className="device-label">凭证</span>
                    <code className="mono" title={device.token}>
                      {device.token}
                    </code>
                    <button
                      className="chip"
                      title={`复制 ${name} 的凭证`}
                      aria-label={`复制 ${name} 的凭证`}
                      onClick={() => void copy(device.token, '凭证')}
                    >
                      复制
                    </button>
                  </div>

                  <div className="device-field">
                    <span className="device-label">订阅</span>
                    <span className="mono ellipsis" title={device.playlist_url}>
                      {device.playlist_url}
                    </span>
                    <button
                      className="chip"
                      title={`复制 ${name} 的订阅地址`}
                      aria-label={`复制 ${name} 的订阅地址`}
                      onClick={() => void copy(device.playlist_url, '订阅地址')}
                    >
                      复制
                    </button>
                  </div>

                  <div className="device-field">
                    <span className="device-label">EPG</span>
                    <span className="mono ellipsis" title={device.epg_url}>
                      {device.epg_url}
                    </span>
                    <button
                      className="chip"
                      title={`复制 ${name} 的 EPG 地址`}
                      aria-label={`复制 ${name} 的 EPG 地址`}
                      onClick={() => void copy(device.epg_url, 'EPG 地址')}
                    >
                      复制
                    </button>
                  </div>

                  <div className="device-meta">
                    <span title={state.detail}>
                      最后使用 {formatTime(device.last_seen_at_ms)}
                    </span>
                    <span>创建 {formatTime(device.created_at_ms)}</span>
                  </div>
                </article>
              );
            })}
          </div>
        )}
      </div>

      {groups.length === 0 && !loading && (
        <div className="card">
          <Empty text="还没有授权任何频道，请联系管理员。" icon="subs" />
        </div>
      )}

      {groups.map((group) => (
        <div className="card flush" key={group.id}>
          <div className="section-title">
            {group.name}
            <span className="muted">{group.channels.length} 个频道</span>
          </div>
          <table className="table">
            <thead>
              <tr>
                <th>频道</th>
                <th>播放地址</th>
                <th className="right">操作</th>
              </tr>
            </thead>
            <tbody>
              {group.channels.map((channel) => (
                <tr key={channel.id}>
                  <td>{channel.name}</td>
                  <td className="mono ellipsis">{channel.url}</td>
                  <td className="right">
                    <button
                      className="chip"
                      title="复制这个频道的播放地址"
                      onClick={() =>
                        void copyText(channel.url).then((done) =>
                          done ? ok('播放地址已复制') : err('复制失败'),
                        )
                      }
                    >
                      复制
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ))}

      {node}
    </>
  );
}
