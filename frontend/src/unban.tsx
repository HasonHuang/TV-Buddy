import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { getToken } from './api';
import { Icon } from './icons';
import { Tip } from './ui';
import './style.css';

/** 自助解封页（`/unban`）。页面可公开加载，动作需要面板会话。 */
function authHeaders(): Record<string, string> {
  const token = getToken();
  return token ? { Authorization: `Bearer ${token}` } : {};
}

function App() {
  const [ip, setIp] = useState('读取中…');
  const [message, setMessage] = useState('');
  const [tone, setTone] = useState<'ok' | 'err' | 'info'>('info');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    fetch('/unban/whoami', { headers: authHeaders() })
      .then(async (response) => {
        if (response.status === 401) {
          setIp('未登录');
          setTone('info');
          setMessage('请先在面板登录后再解封。');
          return null;
        }
        return response.json();
      })
      .then((body) => {
        if (body) {
          setIp(body.ip ?? 'unknown');
        }
      })
      .catch(() => setIp('unknown'));
  }, []);

  async function submit() {
    setBusy(true);
    setMessage('');
    try {
      const response = await fetch('/unban', {
        method: 'POST',
        headers: authHeaders(),
      });
      if (response.status === 401) {
        setTone('err');
        setMessage('请先在面板登录后再解封。');
        return;
      }
      const body = await response.json();
      if (!response.ok) {
        setTone('err');
        setMessage(body.error ?? '解封失败');
      } else if (body.ok) {
        setTone('ok');
        setMessage(`已解封。本月剩余 ${body.remaining} 次。`);
      } else {
        setTone('info');
        setMessage(`当前地址未被封禁。本月剩余 ${body.remaining} 次。`);
      }
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
        </p>

        <button
          type="button"
          title="为当前地址提交一次解封请求"
          onClick={submit}
          disabled={busy}
        >
          {busy ? '提交中…' : '提交解封'}
        </button>
        <div className="unban-tip">
          <Tip text="解封只影响当前网络地址。次数用完后需要等待或联系管理员。" />
        </div>

        {message ? <p className={`msg ${tone}`}>{message}</p> : null}
        {!getToken() ? (
          <p className="muted">
            <a href="/panel">前往面板登录</a>
          </p>
        ) : null}
      </div>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
