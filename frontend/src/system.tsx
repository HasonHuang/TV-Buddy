/** 系统状态：健康摘要、ffmpeg、解析脚本与回看缓冲。 */

import { useCallback, useEffect, useState } from 'react';

import { api } from './api';
import { Icon } from './icons';
import { Badge, PageHead, useToast } from './ui';

type HealthEntry = {
  checked_at: number;
  consecutive_fails: number;
  error: string | null;
  http_status: number | null;
  latency_ms: number | null;
  ok: boolean;
};

type Health = {
  ok: boolean;
  enabled?: boolean;
  entries?: Record<string, HealthEntry>;
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

/** `/panel/api/dash/status` 的单条会话。 */
type DashSession = {
  stream_key: string;
  phase: string;
  segment_count: number;
  last_error: string | null;
  uptime_secs: number;
  active_tracks?: { name: string; kind: string }[];
};

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
  const [dash, setDash] = useState<DashSession[]>([]);
  const [scanUrl, setScanUrl] = useState('');
  const [scan, setScan] = useState<{ kind: string; count: number; tracks: unknown[] } | null>(
    null,
  );
  const { node, ok, err } = useToast();

  const load = useCallback(async () => {
    try {
      const [nextHealth, nextFfmpeg, nextResolvers, nextTimeshift, nextDash] = await Promise.all([
        api<Health>('GET', '/health'),
        api<{ installed: boolean; version: string | null }>('GET', '/assets/ffmpeg-status'),
        api<ResolverList>('GET', '/resolver/scripts'),
        api<Timeshift>('GET', '/dash-timeshift/status'),
        api<DashSession[]>('GET', '/dash/status').catch(() => []),
      ]);
      setHealth(nextHealth);
      setFfmpeg(nextFfmpeg);
      setResolvers(nextResolvers);
      setTimeshift(nextTimeshift);
      setDash(Array.isArray(nextDash) ? nextDash : []);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  /** 删除解析脚本：`POST /resolver/remove {name}`。 */
  const removeResolver = async (name: string) => {
    if (!window.confirm(`确认删除解析脚本「${name}」？`)) return;
    try {
      await api('POST', '/resolver/remove', { name });
      ok('解析脚本已删除');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  useEffect(() => {
    void load();
  }, [load]);

  const buffered = timeshift?.channels.reduce((sum, channel) => sum + channel.segments, 0) ?? 0;

  /** 扫描任意 MPD：`POST /dash/scan-url {url}`，只解析不改配置。 */
  const scanMpd = async () => {
    const url = scanUrl.trim();
    if (url === '') {
      err('请填写 MPD 地址');
      return;
    }
    try {
      const data = await api<{ kind: string; count: number; tracks: unknown[] }>(
        'POST',
        '/dash/scan-url',
        { url },
      );
      setScan(data);
      ok(`已解析 ${data.count} 条轨道`);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  return (
    <>
      <PageHead icon="overview" title="系统状态" sub="健康摘要、依赖与探测结果">
        <button className="ghost" onClick={() => void load()}>
          <Icon name="refresh" size={14} />
          刷新
        </button>
      </PageHead>
      <div className="card">
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
              <>
                <Badge tone="ok">已安装</Badge>
                {ffmpeg.version ? <span className="muted mono version"> {ffmpeg.version}</span> : null}
              </>
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
              <div key={script.name} className="row between">
                <span className="muted">
                  {script.name}（{Math.round(script.bytes / 1024)} KB）
                </span>
                <button
                  className="ghost danger-text"
                  onClick={() => void removeResolver(script.name)}
                >
                  删除
                </button>
              </div>
            ))}
          </dd>
        </dl>
      </div>

      <div className="card">
        <h2>DASH 会话</h2>
        {dash.length === 0 ? (
          <p className="muted">当前没有运行中的 DASH 会话。</p>
        ) : (
          <table className="table">
            <thead>
              <tr>
                <th>会话</th>
                <th>阶段</th>
                <th className="right">切片</th>
                <th className="right">运行</th>
                <th>轨道</th>
              </tr>
            </thead>
            <tbody>
              {dash.map((session) => (
                <tr key={session.stream_key}>
                  <td className="mono">{session.stream_key}</td>
                  <td>
                    <Badge tone={session.phase === 'ready' ? 'ok' : 'warn'}>
                      {session.phase}
                    </Badge>
                    {session.last_error ? (
                      <span className="muted"> {session.last_error}</span>
                    ) : null}
                  </td>
                  <td className="right">{session.segment_count}</td>
                  <td className="right">{Math.round(session.uptime_secs)} 秒</td>
                  <td className="mono">
                    {(session.active_tracks ?? []).map((track) => track.name).join(', ') || '—'}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
        <div className="row" style={{ marginTop: 12 }}>
          <input
            placeholder="MPD 地址，例如 https://host/live.mpd"
            value={scanUrl}
            onChange={(event) => setScanUrl(event.target.value)}
            style={{ flex: 1 }}
          />
          <button className="ghost" onClick={() => void scanMpd()}>
            扫描轨道
          </button>
        </div>
        {scan && (
          <pre className="mono" style={{ whiteSpace: 'pre-wrap', marginTop: 8 }}>
            {JSON.stringify(scan.tracks, null, 2)}
          </pre>
        )}
      </div>

      <div className="card">
        <h2>频道探测</h2>
        {health?.enabled === false ? (
          <p className="muted">未启用（<code className="mono">[health].enabled = false</code>）。</p>
        ) : (
          <dl>
            {Object.entries(health?.entries ?? {}).map(([key, entry]) => (
              <div key={key} style={{ display: 'contents' }}>
                <dt className="mono">{key}</dt>
                <dd>
                  <Badge tone={entry.ok ? 'ok' : 'warn'}>
                    {entry.ok ? '正常' : '异常'}
                  </Badge>{' '}
                  <span className="muted">
                    {entry.latency_ms === null ? '—' : `${entry.latency_ms} ms`}
                    {entry.http_status === null ? '' : ` / HTTP ${entry.http_status}`}
                    {entry.error ? ` / ${entry.error}` : ''}
                    {entry.consecutive_fails > 0 ? ` / 连败 ${entry.consecutive_fails}` : ''}
                  </span>
                </dd>
              </div>
            ))}
          </dl>
        )}
      </div>

      {node}
    </>
  );
}
