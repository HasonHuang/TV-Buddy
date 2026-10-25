import { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { api, clearToken, getToken, setToken, UNAUTHORIZED_EVENT } from './api';
import { useToast } from './ui';
import { GroupsPage } from './groups';
import { EpgPanelPage, LogsPage, SecurityPage } from './ops';
import { CachePage, StatsPage } from './cache';
import { AssetsPage } from './assets';
import { SettingsPage } from './settings';
import { LogosPage } from './logo';
import { PlaylistPage } from './playlist';
import { LivestreamPage, SchedulePage } from './schedule';
import './style.css';

/** 侧栏分区；顺序按目标面板。 */
type SectionId =
  | 'dashboard'
  | 'cache'
  | 'groups'
  | 'settings'
  | 'security'
  | 'stats'
  | 'schedule'
  | 'epg'
  | 'logos'
  | 'subs'
  | 'livestream'
  | 'assets'
  | 'logs';

const NAV: { id: SectionId; label: string }[] = [
  { id: 'dashboard', label: '概览' },
  { id: 'cache', label: '缓存状态' },
  { id: 'groups', label: '分组管理' },
  { id: 'settings', label: '系统设置' },
  { id: 'security', label: '安全管控' },
  { id: 'stats', label: '频道热度' },
  { id: 'schedule', label: '定时录制' },
  { id: 'epg', label: '节目指南' },
  { id: 'logos', label: '台标管理' },
  { id: 'subs', label: '订阅聚合' },
  { id: 'livestream', label: '回看管理' },
  { id: 'assets', label: '视频素材' },
  { id: 'logs', label: '运行日志' },
];

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
  const [error, setError] = useState('');
  const [busy, setBusy] = useState('');
  const { node, ok, err } = useToast();

  const load = useCallback(async () => {
    try {
      setStatus(await api<Status>('GET', '/status'));
      setError('');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  /** 快速操作：端点都是现成的，成功后刷新状态。 */
  const action = async (name: string, path: string, done: string) => {
    setBusy(name);
    try {
      await api('POST', path);
      ok(done);
      await load();
    } catch (cause) {
      err(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy('');
    }
  };

  const entries = Object.entries(status ?? {}).filter(([, value]) => typeof value !== 'object');

  return (
    <div className="card">
      <div className="row between">
        <div>
          <h1>IPTV Proxy</h1>
          <h2>概览</h2>
        </div>
        <div className="row">
          <button className="ghost" disabled={busy !== ''} onClick={() => void load()}>
            刷新状态
          </button>
          <button
            className="ghost"
            disabled={busy !== ''}
            onClick={() => void action('flush', '/cache/flush', '本地缓存已清空')}
          >
            {busy === 'flush' ? '清理中…' : '清空缓存'}
          </button>
          <button
            className="ghost"
            disabled={busy !== ''}
            onClick={() => void action('reload', '/reload', '配置已热重载')}
          >
            {busy === 'reload' ? '重载中…' : '热重载配置'}
          </button>
          <a className="ghost" href="/playlist.m3u" target="_blank" rel="noreferrer">
            导出全部 M3U
          </a>
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
      {error && <div className="msg err">{error}</div>}
      {node}
    </div>
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
        </nav>
        <div className="sidebar-foot">
          <button className="ghost" onClick={() => void logout()}>
            退出登录
          </button>
        </div>
      </aside>
      <main className="content">
        {section === 'dashboard' && <Overview />}
        {section === 'cache' && <CachePage />}
        {section === 'groups' && <GroupsPage />}
        {section === 'settings' && <SettingsPage />}
        {section === 'security' && <SecurityPage />}
        {section === 'stats' && <StatsPage />}
        {section === 'schedule' && <SchedulePage view="schedule" />}
        {section === 'epg' && <EpgPanelPage />}
        {section === 'logos' && <LogosPage />}
        {section === 'subs' && <PlaylistPage />}
        {section === 'livestream' && <LivestreamPage />}
        {section === 'assets' && <AssetsPage />}
        {section === 'logs' && <LogsPage />}
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
