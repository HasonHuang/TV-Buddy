/**
 * 节目单浏览页（P1）。
 *
 * 数据来自 `/epg/api/*`，无需登录。页面只做浏览与搜索，
 * 不写入任何配置。
 */

import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import './style.css';

type Channel = {
  id: string;
  names: { lang: string | null; name: string }[];
};

type Programme = {
  channel: string;
  start: string;
  stop: string;
  title: string;
  desc: string;
  categories: string[];
};

/** 频道显示名：优先第一个 `display-name`，否则用 id。 */
function channelName(channel: Channel): string {
  return channel.names[0]?.name || channel.id;
}

/** `20261020080000 +0800` → `10-20 08:00`。 */
function formatTime(value: string): string {
  const digits = value.slice(0, 12);
  if (digits.length < 12) {
    return value;
  }
  return `${digits.slice(4, 6)}-${digits.slice(6, 8)} ${digits.slice(8, 10)}:${digits.slice(10, 12)}`;
}

function App() {
  const [channels, setChannels] = useState<Channel[]>([]);
  const [active, setActive] = useState('');
  const [programmes, setProgrammes] = useState<Programme[]>([]);
  const [query, setQuery] = useState('');
  const [results, setResults] = useState<Programme[] | null>(null);
  const [error, setError] = useState('');

  // 频道表：进入页面取一次。
  useEffect(() => {
    fetch('/epg/api/channels')
      .then((response) => {
        if (!response.ok) {
          throw new Error(`HTTP ${response.status}`);
        }
        return response.json();
      })
      .then((data: { channels?: Channel[] }) => {
        const list = data.channels ?? [];
        setChannels(list);
        setActive(list[0]?.id ?? '');
      })
      .catch(() => setError('节目单尚未生成。请稍后再试。'));
  }, []);

  // 选中频道的节目。
  useEffect(() => {
    if (!active) {
      return;
    }
    fetch(`/epg/api/programmes/${encodeURIComponent(active)}`)
      .then((response) => (response.ok ? response.json() : { programmes: [] }))
      .then((data: { programmes?: Programme[] }) => setProgrammes(data.programmes ?? []))
      .catch(() => setProgrammes([]));
  }, [active]);

  // 搜索：输入后 200 ms 触发。
  useEffect(() => {
    const text = query.trim();
    if (!text) {
      setResults(null);
      return;
    }
    const timer = window.setTimeout(() => {
      fetch(`/epg/api/search?q=${encodeURIComponent(text)}`)
        .then((response) => (response.ok ? response.json() : { programmes: [] }))
        .then((data: { programmes?: Programme[] }) => setResults(data.programmes ?? []))
        .catch(() => setResults([]));
    }, 200);
    return () => window.clearTimeout(timer);
  }, [query]);

  const names = new Map(channels.map((channel) => [channel.id, channelName(channel)]));

  return (
    <div className="page">
      <div className="card">
        <div className="row" style={{ justifyContent: 'space-between', alignItems: 'center' }}>
          <h1>节目单</h1>
          <input
            className="field"
            placeholder="搜索节目"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
            style={{ maxWidth: 260 }}
          />
        </div>
        {error && <p className="danger-text">{error}</p>}
        {!error && channels.length === 0 && <p className="muted">加载中…</p>}
      </div>

      <div className="shell" style={{ marginTop: 16 }}>
        <aside className="sidebar" style={{ minWidth: 180 }}>
          {channels.map((channel) => (
            <button
              key={channel.id}
              className={`nav-item${channel.id === active ? ' active' : ''}`}
              onClick={() => {
                setQuery('');
                setActive(channel.id);
              }}
              style={{ width: '100%', textAlign: 'left' }}
            >
              {channelName(channel)}
            </button>
          ))}
        </aside>

        <div className="content">
          <div className="card">
            <h2 style={{ fontSize: 16, marginTop: 0 }}>
              {results ? `搜索结果（${results.length}）` : names.get(active) ?? ''}
            </h2>
            <ul style={{ listStyle: 'none', padding: 0, margin: 0 }}>
              {(results ?? programmes).map((programme, index) => (
                <li
                  key={`${programme.channel}-${programme.start}-${index}`}
                  style={{ padding: '8px 0', borderBottom: '1px solid var(--line, #2a2a2a)' }}
                >
                  <div className="row" style={{ gap: 12, alignItems: 'baseline' }}>
                    <span className="mono muted nowrap">
                      {formatTime(programme.start)} – {formatTime(programme.stop).slice(6)}
                    </span>
                    <span>{programme.title || '（无标题）'}</span>
                    {results && <span className="tag">{names.get(programme.channel) ?? programme.channel}</span>}
                  </div>
                  {programme.desc && <p className="muted ellipsis" style={{ margin: '4px 0 0' }}>{programme.desc}</p>}
                </li>
              ))}
            </ul>
            {(results ?? programmes).length === 0 && <p className="muted">没有节目。</p>}
          </div>
        </div>
      </div>
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
