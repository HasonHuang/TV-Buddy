import { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { api, clearToken, getToken, setToken, UNAUTHORIZED_EVENT } from './api';
import { GroupsPage } from './groups';
import { BackupPage, EpgPanelPage, LogsPage, SecurityPage } from './ops';
import { CachePage, StatsPage } from './cache';
import { AssetsPage } from './assets';
import { PlaylistPage } from './playlist';
import { SchedulePage } from './schedule';
import './style.css';

/** 侧栏分区。已实现的分区可切换，其余显示为待实现。 */
type SectionId =
  | 'dashboard'
  | 'groups'
  | 'cache'
  | 'stats'
  | 'epg'
  | 'assets'
  | 'security'
  | 'schedule'
  | 'playlist'
  | 'logs'
  | 'backup';

const NAV: { id: SectionId; label: string }[] = [
  { id: 'dashboard', label: '概览' },
  { id: 'groups', label: '分组管理' },
  { id: 'cache', label: '缓存状态' },
  { id: 'stats', label: '频道热度' },
  { id: 'epg', label: '节目指南' },
  { id: 'assets', label: '视频素材' },
  { id: 'security', label: '安全管控' },
  { id: 'schedule', label: '录制与回看' },
  { id: 'playlist', label: '订阅聚合' },
  { id: 'logs', label: '运行日志' },
  { id: 'backup', label: '备份恢复' },
];

const PENDING: string[] = [];

type Status = Record<string, unknown>;

function Login({ onDone }: { onDone: () => void }) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError('');
    try {
      const data = await api<{ token: string }>('POST', '/login', { username, password });
      setToken(data.token);
      onDone();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="page">
      <div className="card">
        <h1>IPTV Proxy</h1>
        <h2>管理面板</h2>
        <form onSubmit={submit}>
          <label htmlFor="user">用户名</label>
          <input id="user" value={username} onChange={(e) => setUsername(e.target.value)} />
          <label htmlFor="pass">密码</label>
          <input
            id="pass"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          <div style={{ marginTop: 16 }}>
            <button type="submit" disabled={busy}>
              登 录
            </button>
          </div>
        </form>
        {error && <div className="msg err">{error}</div>}
      </div>
    </div>
  );
}

function Overview() {
  const [status, setStatus] = useState<Status | null>(null);
  const [config, setConfig] = useState('');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const [nextStatus, nextConfig] = await Promise.all([
        api<Status>('GET', '/status'),
        api<unknown>('GET', '/config'),
      ]);
      setStatus(nextStatus);
      setConfig(JSON.stringify(nextConfig, null, 2));
      setError('');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    setMessage('');
    setError('');
    try {
      const body = JSON.parse(config) as unknown;
      await api('PUT', '/config', body);
      setMessage('配置已保存');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  };

  const entries = Object.entries(status ?? {}).filter(([, value]) => typeof value !== 'object');

  return (
    <>
      <div className="card">
        <div className="row between">
          <div>
            <h1>IPTV Proxy</h1>
            <h2>概览</h2>
          </div>
          <div className="row">
            <button className="ghost" onClick={() => void load()}>
              刷新
            </button>
          </div>
        </div>
        <dl>
          {entries.map(([key, value]) => (
            <div key={key} style={{ display: 'contents' }}>
              <dt>{key}</dt>
              <dd>{String(value)}</dd>
            </div>
          ))}
        </dl>
      </div>

      <div className="card">
        <h2>配置（config.toml 全量）</h2>
        <textarea value={config} onChange={(e) => setConfig(e.target.value)} spellCheck={false} />
        <div style={{ marginTop: 12 }}>
          <button onClick={() => void save()}>保存</button>
        </div>
        {message && <div className="msg ok">{message}</div>}
        {error && <div className="msg err">{error}</div>}
      </div>
    </>
  );
}

function Shell({ onLogout }: { onLogout: () => void }) {
  const [section, setSection] = useState<SectionId>('dashboard');

  const logout = async () => {
    try {
      await api('POST', '/logout');
    } catch {
      // 忽略注销错误
    }
    clearToken();
    onLogout();
  };

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <strong>IPTV Proxy</strong>
          <span className="muted">管理面板</span>
        </div>
        <nav>
          {NAV.map((item) => (
            <button
              key={item.id}
              className={`nav-item${section === item.id ? ' active' : ''}`}
              onClick={() => setSection(item.id)}
            >
              {item.label}
            </button>
          ))}
          {PENDING.map((label) => (
            <button key={label} className="nav-item" disabled>
              {label}
              <span className="tag">待实现</span>
            </button>
          ))}
        </nav>
        <div className="sidebar-foot">
          <button className="ghost" onClick={() => void logout()}>
            退出登录
          </button>
        </div>
      </aside>
      <main className="content">
        {section === 'groups' && <GroupsPage />}
        {section === 'cache' && <CachePage />}
        {section === 'stats' && <StatsPage />}
        {section === 'epg' && <EpgPanelPage />}
        {section === 'assets' && <AssetsPage />}
        {section === 'security' && <SecurityPage />}
        {section === 'schedule' && <SchedulePage />}
        {section === 'playlist' && <PlaylistPage />}
        {section === 'logs' && <LogsPage />}
        {section === 'backup' && <BackupPage />}
        {section === 'dashboard' && <Overview />}
      </main>
    </div>
  );
}

function App() {
  const [loggedIn, setLoggedIn] = useState(() => getToken() !== '');

  // 会话失效（401）时回到登录卡。
  useEffect(() => {
    const onUnauthorized = () => setLoggedIn(false);
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
  }, []);

  return loggedIn ? (
    <Shell onLogout={() => setLoggedIn(false)} />
  ) : (
    <Login onDone={() => setLoggedIn(true)} />
  );
}

createRoot(document.getElementById('root')!).render(<App />);
