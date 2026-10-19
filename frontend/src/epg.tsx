import { createRoot } from 'react-dom/client';

import './style.css';

/**
 * 节目单页（`/epg`）。
 *
 * M0 只放骨架。数据面在 P1 接入 `/epg.xml` 与 `/panel/api/epg/*`（S-EPG-01、S-EP-05）。
 */
function App() {
  return (
    <div className="page">
      <div className="card">
        <h1>节目单</h1>
        <p style={{ color: 'var(--text2)' }}>
          骨架页。P1 接入 XMLTV 数据与频道匹配结果。
        </p>
      </div>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
