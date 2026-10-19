import { createRoot } from 'react-dom/client';

import './style.css';

/**
 * 台标库页（`/logo`）。
 *
 * M0 只放骨架。数据面在 P2 接入 `/logo/api/list` 与 `/logo/<file>`（S-EP-06、S-LOGO-01）。
 */
function App() {
  return (
    <div className="page">
      <div className="card">
        <h1>台标库</h1>
        <p style={{ color: 'var(--text2)' }}>骨架页。P2 接入台标列表与公开取图。</p>
      </div>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
