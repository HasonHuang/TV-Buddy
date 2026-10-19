import { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { api, clearToken, getToken, setToken } from './api';
import './style.css';

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

function Dashboard({ onLogout }: { onLogout: () => void }) {
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

  const logout = async () => {
    try {
      await api('POST', '/logout');
    } catch {
      // 忽略注销错误
    }
    clearToken();
    onLogout();
  };

  const entries = Object.entries(status ?? {}).filter(([, value]) => typeof value !== 'object');

  return (
    <div className="page">
      <div className="card">
        <div className="row" style={{ justifyContent: 'space-between' }}>
          <div>
            <h1>IPTV Proxy</h1>
            <h2>概览</h2>
          </div>
          <div className="row">
            <button className="ghost" onClick={() => void load()}>
              刷新
            </button>
            <button className="ghost" onClick={() => void logout()}>
              退出登录
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
    </div>
  );
}

function App() {
  const [loggedIn, setLoggedIn] = useState(() => getToken() !== '');
  return loggedIn ? (
    <Dashboard onLogout={() => setLoggedIn(false)} />
  ) : (
    <Login onDone={() => setLoggedIn(true)} />
  );
}

createRoot(document.getElementById('root')!).render(<App />);
