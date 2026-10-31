/**
 * 「我的订阅」分区。
 *
 * 普通用户登录后只看到这一页。数据来自 `GET /panel/api/me`：
 * 订阅地址、EPG 地址与可见的分组/频道。
 * 管理员也看这一页，范围是自己的全部频道。
 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { Icon } from './icons';
import { PageHead, useToast } from './ui';

type Channel = { id: string; name: string; url: string };
type Group = { id: string; name: string; channels: Channel[] };

type Me = {
  user?: { username?: string; role?: string };
  token?: string;
  playlist_link?: string | null;
  epg_link?: string | null;
  groups?: Group[];
};

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
  const total = groups.reduce((sum, group) => sum + group.channels.length, 0);

  return (
    <>
      <PageHead
        icon="subs"
        title="我的订阅"
        sub={`${groups.length} 个分组 · ${total} 个频道`}
      >
        <button className="ghost" onClick={() => void load()} disabled={loading}>
          <Icon name="refresh" size={14} />
          {loading ? '刷新中…' : '刷新'}
        </button>
      </PageHead>

      <div className="card">
        <h2>订阅地址</h2>
        <p className="muted">
          把这个地址填进播放器的 M3U 订阅。地址里带你的专属令牌，不要外传。
        </p>
        <div className="row form-row">
          <input readOnly value={me?.playlist_link ?? ''} className="mono" />
          <button className="chip" onClick={() => void copy(me?.playlist_link, '订阅地址')}>
            复制
          </button>
        </div>
        {me?.epg_link && (
          <>
            <h2>EPG 地址</h2>
            <p className="muted">只含你可见频道的节目单。播放器支持时填这里。</p>
            <div className="row form-row">
              <input readOnly value={me.epg_link} className="mono" />
              <button className="chip" onClick={() => void copy(me.epg_link, 'EPG 地址')}>
                复制
              </button>
            </div>
          </>
        )}
      </div>

      {groups.length === 0 && !loading && (
        <div className="card muted">还没有授权任何频道。请联系管理员。</div>
      )}

      {groups.map((group) => (
        <div className="card flush" key={group.id}>
          <div className="section-title">{group.name}</div>
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
