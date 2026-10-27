import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import './style.css';

/** 自助解封页（`/unban`）。免登录（S-EP-07）。 */
function App() {
  const [ip, setIp] = useState('读取中…');
  const [message, setMessage] = useState('');
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    fetch('/unban/whoami')
      .then((response) => response.json())
      .then((body) => setIp(body.ip ?? 'unknown'))
      .catch(() => setIp('unknown'));
  }, []);

  async function submit() {
    setBusy(true);
    setMessage('');
    try {
      const response = await fetch('/unban', { method: 'POST' });
      const body = await response.json();
      if (!response.ok) {
        setMessage(body.error ?? '解封失败');
      } else if (body.ok) {
        setMessage(`已解封。本月剩余 ${body.remaining} 次。`);
      } else {
        setMessage(`当前地址未被封禁。本月剩余 ${body.remaining} 次。`);
      }
    } catch {
      setMessage('请求失败，请稍后重试。');
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="page">
      <div className="card">
        <h1>自助解封</h1>
        <p style={{ color: 'var(--text2)' }}>当前地址：{ip}</p>
        <p style={{ color: 'var(--text2)' }}>每个地址 30 天最多解封 3 次。</p>
        <button onClick={submit} disabled={busy}>
          {busy ? '提交中…' : '提交解封'}
        </button>
        {message && <p style={{ marginTop: 12 }}>{message}</p>}
      </div>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
