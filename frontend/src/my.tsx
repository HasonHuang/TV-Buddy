/**
 * 「我的订阅」分区。
 *
 * 普通用户登录后只看到这一页。数据来自 `GET /api/panel/me`：
 * 订阅地址、EPG 地址与可见的分组/频道。
 * 管理员也看这一页，范围是自己的全部频道。
 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
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
function deviceState(device: Device): { text: string; tone: string } {
  if (device.expires_at_ms === null) return { text: '永不过期', tone: 'ok' };
  if (device.expires_at_ms <= Date.now()) return { text: '已过期', tone: 'danger' };
  return { text: `有效期至 ${formatTime(device.expires_at_ms)}`, tone: 'info' };
}

export function MySubsPage() {
  const { node, ok, err } = useToast();
  const [me, setMe] = useState<Me | null>(null);
  const [loading, setLoading] = useState(true);

  const load = useCallback(async () => {
    setLoading(true);
    try {
      setMe(await api<Me>('GET', '/me'));
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
    const done = await copyText(`${location.origin}${value}`);
    if (done) ok(`${label}已复制`);
    else err('复制失败，请手动选取');
  };

  const groups = me?.groups ?? [];
  const devices = me?.devices ?? [];
  const total = groups.reduce((sum, group) => sum + group.channels.length, 0);

  const head = (
    <PageHead
      icon="subs"
      title="我的订阅"
      sub={
        loading && me === null
          ? '正在读取订阅信息'
          : `${groups.length} 个分组 · ${total} 个频道`
      }
    >
      <a
        className="ghost"
        href="/page/unban"
        title="当前地址被封禁时提交解封"
      >
        <Icon name="security" size={14} />
        自助解封
      </a>
      <button
        className="ghost"
        title="重新读取你的订阅地址与授权频道"
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

      <div className="card">
        <h2>
          订阅地址
          <Tip text="地址里带你的专属凭证。任何人拿到它都能播放你看得到的频道。" />
        </h2>
        <p className="muted">
          把这个地址填进播放器的订阅。地址里带你的专属凭证，不要外传。
        </p>
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
        {me?.epg_link && (
          <>
            <h2>
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
        )}
      </div>

      {groups.length === 0 && !loading && (
        <div className="card">
          <Empty text="还没有授权任何频道，请联系管理员。" icon="subs" />
        </div>
      )}

      <div className="card flush">
        <div className="section-title">
          我的设备
          <span className="muted">{devices.length} 台</span>
        </div>
        <p className="muted" style={{ padding: '0 16px' }}>
          设备由管理员管理。这里只能查看与复制。需要添加或撤销设备，请联系管理员。
        </p>
        {devices.length === 0 ? (
          <Empty text="还没有设备。" icon="users" />
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>设备</th>
                <th>凭证</th>
                <th>状态</th>
                <th>最后使用</th>
                <th className="right">订阅地址</th>
              </tr>
            </thead>
            <tbody>
              {devices.map((device) => {
                const state = deviceState(device);
                return (
                  <tr key={device.token}>
                    <td>{device.label || '未命名设备'}</td>
                    <td className="mono">{device.token}</td>
                    <td>
                      <Badge tone={state.tone}>{state.text}</Badge>
                    </td>
                    <td className="muted">{formatTime(device.last_seen_at_ms)}</td>
                    <td className="right">
                      <button
                        className="chip"
                        title="复制这台设备的订阅地址"
                        onClick={() => void copy(device.playlist_url, '订阅地址')}
                      >
                        订阅
                      </button>
                      <button
                        className="chip"
                        title="复制这台设备的 EPG 地址"
                        onClick={() => void copy(device.epg_url, 'EPG 地址')}
                      >
                        EPG
                      </button>
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        )}
      </div>

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
                      onClick={() => void copyText(channel.url).then((done) =>
                        done ? ok('播放地址已复制') : err('复制失败'),
                      )}
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
