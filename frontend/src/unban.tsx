import { createRoot } from 'react-dom/client';

import './style.css';

/**
 * 自助解封页（`/unban`）。
 *
 * 免登录（S-EP-07）。M0 只放骨架；接口在 P1 接入 `/unban/whoami` 与解封提交。
 */
function App() {
  return (
    <div className="page">
      <div className="card">
        <h1>自助解封</h1>
        <p style={{ color: 'var(--text2)' }}>骨架页。P1 接入本机地址查询与解封提交。</p>
      </div>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
