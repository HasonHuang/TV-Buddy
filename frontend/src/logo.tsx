/**
 * 台标库页（`/logo`）。
 *
 * 公开浏览；登录后可上传与删除（`/panel/api/logo/*`）。
 */

import { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { api, getToken } from './api';
import './style.css';

type Logo = {
  file: string;
  name: string;
  ext: string;
  bytes: number;
  url: string;
};

type UploadResult = {
  added: Logo[];
  failed: { name: string; error: string }[];
};

function formatBytes(bytes: number): string {
  if (bytes < 1024) {
    return `${bytes} B`;
  }
  if (bytes < 1024 * 1024) {
    return `${(bytes / 1024).toFixed(1)} KB`;
  }
  return `${(bytes / 1024 / 1024).toFixed(1)} MB`;
}

function readAsDataUrl(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(String(reader.result));
    reader.onerror = () => reject(reader.error);
    reader.readAsDataURL(file);
  });
}

function App() {
  const [logos, setLogos] = useState<Logo[]>([]);
  const [message, setMessage] = useState('');
  const [authed] = useState(() => Boolean(getToken()));

  const load = useCallback(async () => {
    const response = await fetch('/logo/api/list');
    if (!response.ok) {
      setMessage(`读取台标库失败：HTTP ${response.status}`);
      return;
    }
    const data = (await response.json()) as { logos?: Logo[] };
    setLogos(data.logos ?? []);
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  async function upload(files: FileList | null) {
    if (!files || files.length === 0) {
      return;
    }
    try {
      const items = await Promise.all(
        Array.from(files).map(async (file) => ({
          name: file.name,
          data: await readAsDataUrl(file),
        })),
      );
      const result = await api<UploadResult>('POST', '/logo/upload', { items });
      setMessage(`已上传 ${result.added.length} 个，失败 ${result.failed.length} 个`);
      await load();
    } catch (error) {
      setMessage((error as Error).message);
    }
  }

  async function remove(file: string) {
    try {
      await api('POST', '/logo/delete', { file });
      await load();
      setMessage('已删除');
    } catch (error) {
      setMessage((error as Error).message);
    }
  }

  return (
    <div className="page">
      <div className="card">
        <div className="row" style={{ justifyContent: 'space-between', alignItems: 'center' }}>
          <h1>台标库</h1>
          {authed && (
            <label className="tag" style={{ cursor: 'pointer' }}>
              上传台标
              <input
                type="file"
                accept="image/*"
                multiple
                style={{ display: 'none' }}
                onChange={(event) => void upload(event.target.files)}
              />
            </label>
          )}
        </div>
        <p className="muted">
          共 {logos.length} 个台标。公开地址形如 <code className="mono">/logo/&lt;文件名&gt;</code>。
        </p>
        {message && <p className="msg">{message}</p>}
      </div>

      <div className="card" style={{ marginTop: 16 }}>
        {logos.length === 0 && <p className="muted">台标库为空。</p>}
        <div
          style={{
            display: 'grid',
            gridTemplateColumns: 'repeat(auto-fill, minmax(140px, 1fr))',
            gap: 12,
          }}
        >
          {logos.map((logo) => (
            <div key={logo.file} style={{ textAlign: 'center' }}>
              <img
                src={logo.url}
                alt={logo.name}
                style={{ maxWidth: '100%', maxHeight: 80, objectFit: 'contain' }}
              />
              <div className="mono ellipsis" style={{ fontSize: 12 }}>
                {logo.file}
              </div>
              <div className="muted" style={{ fontSize: 12 }}>
                {formatBytes(logo.bytes)}
              </div>
              {authed && (
                <button className="icon-btn" onClick={() => void remove(logo.file)}>
                  删除
                </button>
              )}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
