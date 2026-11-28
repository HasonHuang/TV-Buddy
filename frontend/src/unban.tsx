import { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { getToken } from './api';
import { Icon } from './icons';
import { Tip } from './ui';
import './style.css';

/** 自助解封页（`/page/unban`）。页面可公开加载，解封动作需要面板登录。 */
function authHeaders(): Record<string, string> {
  const token = getToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

function App() {
  const [ip, setIp] = useState('读取中…');
  const [message, setMessage] = useState('');
  const [tone, setTone] = useState<'ok' | 'err' | 'info'>('info');
  const [busy, setBusy] = useState(false);
  const [loggedIn, setLoggedIn] = useState(() => getToken() !== '');

  const load = useCallback(async () => {
    try {
      const response = await fetch('/api/unban/whoami', { headers: authHeaders() });
      if (response.status === 401) {
        setLoggedIn(false);
        setIp('未登录');
        setTone('info');
        setMessage('解封需要登录。请先在面板登录，然后回到本页。');
        return;
      }
      const body = await response.json();
      setLoggedIn(true);
      setIp(body.ip ?? 'unknown');
    } catch {
      setIp('unknown');
      setTone('err');
      setMessage('读取当前地址失败，请稍后重试。');
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function submit() {
    setBusy(true);
    setMessage('');
    try {
      const response = await fetch('/api/unban', {
        method: 'POST',
        headers: authHeaders(),
      });
      if (response.status === 401) {
        setLoggedIn(false);
        setTone('err');
        setMessage('解封需要登录。请先在面板登录，然后回到本页。');
        return;
      }
      const body = await response.json();
      const remaining =
        typeof body.remaining === 'number' ? `本月剩余 ${body.remaining} 次。` : '';
      if (!response.ok) {
        setTone('err');
        setMessage(body.error ?? '解封失败。');
      } else if (body.ok) {
        setTone('ok');
        setMessage(`解封成功。${remaining}`);
      } else {
        setTone('info');
        setMessage(`${body.msg ?? '当前地址未被封禁。'}${remaining}`);
      }
      await load();
    } catch {
      setTone('err');
      setMessage('请求失败，请稍后重试。');
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="login-wrap">
      <div className="login-card unban-card">
        <div className="mark">
          <Icon name="security" size={34} />
        </div>
        <div className="name">自助解封</div>
        <div className="tagline">恢复访问</div>

        <div className="unban-ip">
          <span className="muted">当前地址</span>
          <code className="mono">{ip}</code>
        </div>

        <p className="muted unban-note">
          如果面板或播放地址被临时封禁，可以在这里申请解封。每个地址 30 天内最多解封 3 次。
          解封前需要登录面板。
        </p>

        <button
          type="button"
          title="为当前地址提交一次解封请求"
          onClick={() => void submit()}
          disabled={busy || !loggedIn}
        >
          {busy ? '提交中…' : '提交解封'}
        </button>
        <div className="unban-tip">
          <Tip text="解封只影响当前网络地址。次数用完后需要等待或联系管理员。" />
        </div>

        {message ? <p className={`msg ${tone}`}>{message}</p> : null}
        {!loggedIn ? (
          <p className="muted">
            <a href="/page/panel">前往面板登录</a>
          </p>
        ) : null}
      </div>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
