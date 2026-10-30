/**
 * 缓存状态与频道热度分区。
 *
 * 接口：`/panel/api/cache/{stats,flush}`、`/panel/api/stats`。
 */

import { useCallback, useEffect, useState } from 'react';

import { api } from './api';
import { Icon } from './icons';
import { RecommendCard, type Recommend } from './lowcfg';
import { PageHead } from './ui';

type CacheStats = {
  backend: string;
  disk_path: string;
  entries: Record<string, number>;
  bytes: number;
  playlist_cache_entries: number;
  recommend: Recommend;
};

type Stats = {
  version: string;
  uptime_secs: number;
  requests: number;
  playlists: number;
  segments: number;
  segment_hits: number;
  upstream_failures: number;
  online_window_secs: number;
  /** 窗口内不同观众数。 */
  online_viewers?: number;
  /** 窗口内有人观看的频道数。 */
  active_channels?: number;
  groups: { id: string; requests: number }[];
  /** 频道排行：只含窗口内有人观看的频道，按在线人数降序。 */
  channels?: { gid: string; cid: string; name: string; online: number; requests: number }[];
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  if (bytes < 1024 * 1024 * 1024) {
    return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
  }
  return `${(bytes / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

function formatUptime(seconds: number): string {
  const days = Math.floor(seconds / 86400);
  const hours = Math.floor((seconds % 86400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days > 0) {
    return `${days} 天 ${hours} 小时`;
  }
  if (hours > 0) {
    return `${hours} 小时 ${minutes} 分`;
  }
  return `${minutes} 分`;
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 缓存状态：占用统计与清空。 */
export function CachePage() {
  const [stats, setStats] = useState<CacheStats | null>(null);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      setStats(await api<CacheStats>('GET', '/cache/stats'));
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const flush = async () => {
    if (!window.confirm('清空缓存会删除已落盘的清单与切片，继续？')) {
      return;
    }
    setMessage('');
    setError('');
    try {
      const result = await api<{ removed: number }>('POST', '/cache/flush');
      setMessage(`已删除 ${result.removed} 个文件`);
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const entries = Object.entries(stats?.entries ?? {});

  return (
    <>
      <PageHead
        icon="cache"
        title="缓存状态"
        sub={stats ? `${stats.backend} · ${formatBytes(stats.bytes)}` : '读取中…'}
      >
        <button className="ghost" onClick={() => void load()}>
          <Icon name="refresh" size={14} />
          刷新
        </button>
        <button onClick={() => void flush()}>清空缓存</button>
      </PageHead>
      {stats && <RecommendCard recommend={stats.recommend} />}
      <div className="card">
        <h2>缓存目录</h2>
        <dl>
          <dt>目录</dt>
          <dd className="mono ellipsis">{stats?.disk_path ?? '—'}</dd>
          <dt>内存清单缓存</dt>
          <dd>{stats?.playlist_cache_entries ?? 0} 项</dd>
        </dl>
        <table className="table">
          <thead>
            <tr>
              <th>子目录</th>
              <th className="right">文件数</th>
            </tr>
          </thead>
          <tbody>
            {entries.map(([name, files]) => (
              <tr key={name}>
                <td className="mono">{name}</td>
                <td className="right">{files}</td>
              </tr>
            ))}
          </tbody>
        </table>
        {message && <div className="msg ok">{message}</div>}
        {error && <div className="msg err">{error}</div>}
      </div>
    </>
  );
}

/** 频道热度：总请求与分组请求排行。 */
export function StatsPage() {
  const [stats, setStats] = useState<Stats | null>(null);
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      setStats(await api<Stats>('GET', '/stats'));
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const groups = [...(stats?.groups ?? [])].sort((a, b) => b.requests - a.requests);

  return (
    <>
      <PageHead icon="stats" title="频道热度" sub={`版本 ${stats?.version ?? '—'}`}>
        <button className="ghost" onClick={() => void load()}>
          <Icon name="refresh" size={14} />
          刷新
        </button>
      </PageHead>
      <div className="stats">
        <div className="stat">
          <div className="k">
            <Icon name="overview" size={13} />
            当前在线
          </div>
          <div className="v">{stats?.online_viewers ?? 0}</div>
          <div className="hint">近 {stats?.online_window_secs ?? 0} 秒去重观众合计</div>
        </div>
        <div className="stat">
          <div className="k">
            <Icon name="livestream" size={13} />
            活跃频道
          </div>
          <div className="v">{stats?.active_channels ?? 0}</div>
          <div className="hint">有人在线观看的频道数</div>
        </div>
        <div className="stat">
          <div className="k">
            <Icon name="cache" size={13} />
            累计清单请求
          </div>
          <div className="v">{stats?.playlists ?? 0}</div>
          <div className="hint">自启动以来全频道合计</div>
        </div>
      </div>
      <div className="card">
        <div className="section-title">热门频道排行</div>
        <table className="table">
          <thead>
            <tr>
              <th>#</th>
              <th>频道</th>
              <th className="right">在线</th>
              <th className="right">累计请求</th>
            </tr>
          </thead>
          <tbody>
            {(stats?.channels ?? []).map((channel, index) => (
              <tr key={`${channel.gid}/${channel.cid}`}>
                <td className="muted">{index + 1}</td>
                <td>
                  {channel.name || channel.cid}
                  <span className="muted mono"> · {channel.gid}/{channel.cid}</span>
                </td>
                <td className="right">{channel.online}</td>
                <td className="right">{channel.requests}</td>
              </tr>
            ))}
            {(stats?.channels ?? []).length === 0 && (
              <tr>
                <td colSpan={4} className="muted">
                  窗口内没有在线观众。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      <div className="card">
        <div className="section-title">运行统计</div>
        <dl>
          <dt>运行时长</dt>
          <dd>{stats ? formatUptime(stats.uptime_secs) : '—'}</dd>
          <dt>总请求</dt>
          <dd>{stats?.requests ?? 0}</dd>
          <dt>订阅请求</dt>
          <dd>{stats?.playlists ?? 0}</dd>
          <dt>切片请求</dt>
          <dd>{stats?.segments ?? 0}</dd>
          <dt>切片命中</dt>
          <dd>{stats?.segment_hits ?? 0}</dd>
          <dt>上游失败</dt>
          <dd>{stats?.upstream_failures ?? 0}</dd>
        </dl>
        <table className="table">
        <thead>
          <tr>
            <th>分组</th>
            <th className="right">请求</th>
          </tr>
        </thead>
        <tbody>
          {groups.map((group) => (
            <tr key={group.id}>
              <td className="mono">{group.id || '（其它）'}</td>
              <td className="right">{group.requests}</td>
            </tr>
          ))}
          {groups.length === 0 && (
            <tr>
              <td colSpan={2} className="muted">
                暂无数据。
              </td>
            </tr>
          )}
        </tbody>
        </table>
        {error && <div className="msg err">{error}</div>}
      </div>
    </>
  );
}
