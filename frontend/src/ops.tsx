/**
 * 运维分区：安全管控、运行日志、备份与恢复。
 *
 * 接口：`/panel/api/security/*`、`/panel/api/logs`、`/panel/api/{backup,restore}`。
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import { api, getToken } from './api';

type TokenInfo = {
  enabled: boolean;
  ttl_hours: number;
  current: string;
  old: string | null;
};

type IpEntry = {
  ip: string;
  requests: number;
  failures: number;
  banned: boolean;
  remaining_secs: number;
};

type IpStats = {
  count: number;
  banned: number;
  entries: IpEntry[];
};

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 安全管控：播放 Token 与 IP 封禁。 */
export function SecurityPage() {
  const [token, setToken] = useState<TokenInfo | null>(null);
  const [stats, setStats] = useState<IpStats | null>(null);
  const [ip, setIp] = useState('');
  const [seconds, setSeconds] = useState('3600');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const [nextToken, nextStats] = await Promise.all([
        api<TokenInfo>('GET', '/security/token'),
        api<IpStats>('GET', '/security/ip/stats'),
      ]);
      setToken(nextToken);
      setStats(nextStats);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const rotate = async () => {
    setMessage('');
    setError('');
    try {
      await api('POST', '/security/token/rotate');
      setMessage('Token 已轮换，旧值保留 1 小时宽限');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const ban = async () => {
    setMessage('');
    setError('');
    try {
      await api('POST', '/security/ip/ban', {
        ip: ip.trim(),
        seconds: Number(seconds) || 0,
      });
      setMessage(`已封禁 ${ip.trim()}`);
      setIp('');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const unban = async (target: string) => {
    setMessage('');
    setError('');
    try {
      await api('POST', '/security/ip/unban', { ip: target });
      setMessage(`已解封 ${target}`);
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  return (
    <>
      <div className="card">
        <div className="row between">
          <div>
            <h1>安全管控</h1>
            <h2>播放 Token</h2>
          </div>
          <div className="row">
            <button className="ghost" onClick={() => void load()}>
              刷新
            </button>
            <button onClick={() => void rotate()}>轮换 Token</button>
          </div>
        </div>
        <dl>
          <dt>校验</dt>
          <dd>{token ? (token.enabled ? '已开启' : '已关闭') : '—'}</dd>
          <dt>有效期</dt>
          <dd>{token ? `${token.ttl_hours} 小时` : '—'}</dd>
          <dt>当前值</dt>
          <dd className="mono ellipsis">{token?.current ?? '—'}</dd>
          <dt>旧值（宽限）</dt>
          <dd className="mono ellipsis">{token?.old ?? '—'}</dd>
        </dl>
        {message && <div className="msg ok">{message}</div>}
        {error && <div className="msg err">{error}</div>}
      </div>

      <div className="card">
        <h2>IP 统计与封禁</h2>
        <p className="muted">
          共 {stats?.count ?? 0} 个地址，其中封禁 {stats?.banned ?? 0} 个。封禁只作用于非面板路径。
        </p>
        <div className="row">
          <input
            placeholder="地址，例如 10.0.0.2"
            value={ip}
            onChange={(event) => setIp(event.target.value)}
            style={{ flex: 2 }}
          />
          <input
            placeholder="秒"
            value={seconds}
            onChange={(event) => setSeconds(event.target.value)}
            style={{ flex: 1 }}
          />
          <button onClick={() => void ban()} disabled={ip.trim() === ''}>
            封禁
          </button>
        </div>
        <table className="table" style={{ marginTop: 12 }}>
          <thead>
            <tr>
              <th>地址</th>
              <th className="right">请求</th>
              <th className="right">失败</th>
              <th>状态</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {(stats?.entries ?? []).map((entry) => (
              <tr key={entry.ip}>
                <td className="mono">{entry.ip}</td>
                <td className="right">{entry.requests}</td>
                <td className="right">{entry.failures}</td>
                <td>{entry.banned ? `封禁中（${entry.remaining_secs} 秒）` : '正常'}</td>
                <td className="right">
                  {entry.banned && (
                    <button className="ghost" onClick={() => void unban(entry.ip)}>
                      解封
                    </button>
                  )}
                </td>
              </tr>
            ))}
            {(stats?.entries ?? []).length === 0 && (
              <tr>
                <td colSpan={5} className="muted">
                  暂无记录。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </>
  );
}

/** 运行日志：轮询 `/panel/api/logs`。 */
export function LogsPage() {
  const [lines, setLines] = useState<string[]>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState('');
  const [limit, setLimit] = useState('200');

  const load = useCallback(async () => {
    try {
      const data = await api<{ total: number; lines: string[] }>(
        'GET',
        `/logs?limit=${Number(limit) || 200}`,
      );
      setLines(data.lines);
      setTotal(data.total);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, [limit]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(timer);
  }, [load]);

  return (
    <div className="card">
      <div className="row between">
        <div>
          <h1>运行日志</h1>
          <h2>最近 {lines.length} 行（共 {total} 行）</h2>
        </div>
        <div className="row">
          <input
            value={limit}
            onChange={(event) => setLimit(event.target.value)}
            style={{ width: 80 }}
          />
          <button className="ghost" onClick={() => void load()}>
            刷新
          </button>
        </div>
      </div>
      {error && <div className="msg err">{error}</div>}
      <pre className="log">{lines.join('\n')}</pre>
    </div>
  );
}

/** 备份与恢复。 */
export function BackupPage() {
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [scope, setScope] = useState('all');
  const file = useRef<HTMLInputElement>(null);

  const download = async () => {
    setMessage('');
    setError('');
    try {
      const response = await fetch(`/panel/api/backup?include=${scope}`, {
        headers: { Authorization: `Bearer ${getToken()}` },
      });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      const blob = await response.blob();
      const disposition = response.headers.get('content-disposition') ?? '';
      const name = /filename="([^"]+)"/.exec(disposition)?.[1] ?? 'iptv-proxy-backup.tar.gz';
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = name;
      anchor.click();
      URL.revokeObjectURL(url);
      setMessage(`已导出 ${name}（${blob.size} 字节）`);
    } catch (err) {
      setError(errorText(err));
    }
  };

  const restore = async (input: HTMLInputElement) => {
    const selected = input.files?.[0];
    if (!selected) {
      return;
    }
    setMessage('');
    setError('');
    try {
      const response = await fetch('/panel/api/restore', {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${getToken()}`,
          'Content-Type': 'application/gzip',
        },
        body: await selected.arrayBuffer(),
      });
      const data = (await response.json()) as { error?: string; report?: { files: string[] } };
      if (!response.ok) {
        throw new Error(data.error ?? `HTTP ${response.status}`);
      }
      setMessage(`已恢复 ${data.report?.files.length ?? 0} 个文件，配置已重载`);
    } catch (err) {
      setError(errorText(err));
    } finally {
      input.value = '';
    }
  };

  return (
    <div className="card">
      <h1>备份与恢复</h1>
      <h2>导出</h2>
      <div className="row">
        <select value={scope} onChange={(event) => setScope(event.target.value)}>
          <option value="all">全部</option>
          <option value="epg-scripts">仅 EPG 脚本</option>
          <option value="resolver">仅解析脚本</option>
        </select>
        <button onClick={() => void download()}>下载备份</button>
      </div>
      <h2 style={{ marginTop: 24 }}>恢复</h2>
      <p className="muted">
        恢复会覆盖 `config.toml`，旧文件另存为 `config.toml.bak`。非法内容不会落盘。
      </p>
      <input
        ref={file}
        type="file"
        accept=".gz,application/gzip"
        onChange={(event) => void restore(event.target)}
      />
      {message && <div className="msg ok">{message}</div>}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}
