/** 系统状态：健康摘要、ffmpeg、解析脚本与回看缓冲。 */

import { useCallback, useEffect, useState } from 'react';

import { api } from './api';
import { Badge, useToast } from './ui';

type Health = {
  ok: boolean;
  version: string;
  uptime_secs: number;
  time: number;
  bind: string;
  groups: number;
  channels: number;
  sessions: number;
  cache?: Record<string, unknown>;
  [key: string]: unknown;
};

type ResolverList = { count: number; scripts: { name: string; bytes: number }[] };

type Timeshift = {
  enabled: boolean;
  ready: boolean;
  channels: { group_id: string; channel_id: string; hours: number; segments: number }[];
};

function uptime(seconds: number): string {
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor((seconds % 86_400) / 3600);
  const minutes = Math.floor((seconds % 3600) / 60);
  if (days) return `${days} 天 ${hours} 小时`;
  if (hours) return `${hours} 小时 ${minutes} 分`;
  return `${minutes} 分`;
}

export function SystemPage() {
  const [health, setHealth] = useState<Health | null>(null);
  const [ffmpeg, setFfmpeg] = useState<{ installed: boolean; version: string | null } | null>(null);
  const [resolvers, setResolvers] = useState<ResolverList | null>(null);
  const [timeshift, setTimeshift] = useState<Timeshift | null>(null);
  const { node, err } = useToast();

  const load = useCallback(async () => {
    try {
      const [nextHealth, nextFfmpeg, nextResolvers, nextTimeshift] = await Promise.all([
        api<Health>('GET', '/health'),
        api<{ installed: boolean; version: string | null }>('GET', '/assets/ffmpeg-status'),
        api<ResolverList>('GET', '/resolver/scripts'),
        api<Timeshift>('GET', '/dash-timeshift/status'),
      ]);
      setHealth(nextHealth);
      setFfmpeg(nextFfmpeg);
      setResolvers(nextResolvers);
      setTimeshift(nextTimeshift);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const buffered = timeshift?.channels.reduce((sum, channel) => sum + channel.segments, 0) ?? 0;

  return (
    <>
      <div className="card">
        <div className="row between">
          <h1>系统状态</h1>
          <button className="ghost" onClick={() => void load()}>
            刷新
          </button>
        </div>
        {health ? (
          <dl>
            <dt>版本</dt>
            <dd>{health.version}</dd>
            <dt>运行时长</dt>
            <dd>{uptime(health.uptime_secs)}</dd>
            <dt>监听</dt>
            <dd>{health.bind}</dd>
            <dt>分组 / 频道</dt>
            <dd>
              {health.groups} / {health.channels}
            </dd>
            <dt>面板会话</dt>
            <dd>{health.sessions}</dd>
            <dt>公开健康检查</dt>
            <dd>
              <code>{location.origin}/health</code>
            </dd>
          </dl>
        ) : (
          <p className="muted">加载中…</p>
        )}
      </div>

      <div className="card">
        <h2>能力</h2>
        <dl>
          <dt>ffmpeg</dt>
          <dd>
            {ffmpeg?.installed ? (
              <Badge tone="ok">已安装 {ffmpeg.version ?? ''}</Badge>
            ) : (
              <Badge tone="warn">未安装（录制只能保留 TS）</Badge>
            )}
          </dd>
          <dt>回看缓冲</dt>
          <dd>
            {timeshift?.enabled ? (
              <Badge tone={timeshift.ready ? 'ok' : 'warn'}>
                {timeshift.channels.length} 个频道 / {buffered} 片
              </Badge>
            ) : (
              <Badge tone="muted">未启用</Badge>
            )}
          </dd>
          <dt>解析脚本</dt>
          <dd>
            {resolvers ? `${resolvers.count} 个` : '—'}
            {resolvers?.scripts.map((script) => (
              <div key={script.name} className="muted">
                {script.name}（{Math.round(script.bytes / 1024)} KB）
              </div>
            ))}
          </dd>
        </dl>
      </div>

      {node}
    </>
  );
}
