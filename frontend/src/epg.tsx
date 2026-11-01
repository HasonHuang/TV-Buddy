/**
 * 节目单浏览页（P1）。
 *
 * 数据来自 `/epg/api/*`，无需登录。页面只做浏览与搜索，
 * 不写入任何配置。
 */

import { useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { Empty, Tip } from './ui';
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
  const [loading, setLoading] = useState(true);

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
      .catch(() => setError('节目单尚未生成。请稍后再试。'))
      .finally(() => setLoading(false));
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
  const shown = results ?? programmes;

  return (
    <div className="public-page">
      <header className="public-head">
        <div className="public-head-text">
          <h1>节目单</h1>
          <p className="muted">
            按频道浏览节目。搜索会跨频道匹配节目名与简介。
          </p>
        </div>
        <label className="public-search">
          <span>
            搜索节目
            <Tip text="关键词会跨频道匹配节目名与简介，结果替换右侧列表。" />
          </span>
          <input
            aria-label="搜索节目"
            placeholder="输入节目名或关键词"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </label>
      </header>

      {error ? (
        <div className="card">
          <Empty text={error} icon="epg" />
        </div>
      ) : (
        <div className="epg-layout">
          <aside className="epg-side">
            <div className="epg-side-title">频道</div>
            <div className="epg-channel-list">
              {channels.map((channel) => (
                <button
                  key={channel.id}
                  className={`epg-channel${channel.id === active ? ' active' : ''}`}
                  title={channelName(channel)}
                  onClick={() => {
                    setQuery('');
                    setActive(channel.id);
                  }}
                >
                  {channelName(channel)}
                </button>
              ))}
              {channels.length === 0 && !loading ? (
                <p className="muted">暂无频道。</p>
              ) : null}
            </div>
          </aside>

          <section className="epg-main">
            <div className="card">
              <div className="row between" style={{ marginBottom: 8 }}>
                <h2 style={{ marginBottom: 0 }}>
                  {results ? `搜索结果（${results.length}）` : names.get(active) ?? '节目单'}
                </h2>
                {query.trim() !== '' ? (
                  <button
                    className="ghost"
                    title="清空搜索并返回频道节目"
                    onClick={() => setQuery('')}
                  >
                    清除搜索
                  </button>
                ) : null}
              </div>

              {loading ? <p className="muted">加载中…</p> : null}
              {!loading && shown.length === 0 ? (
                <Empty
                  text={results ? '没有匹配的节目。' : '该频道暂无节目。'}
                  icon="epg"
                />
              ) : (
                <ul className="epg-list">
                  {shown.map((programme, index) => (
                    <li
                      key={`${programme.channel}-${programme.start}-${index}`}
                      className="epg-item"
                    >
                      <div className="epg-time mono">
                        {formatTime(programme.start)} – {formatTime(programme.stop).slice(6)}
                      </div>
                      <div className="epg-body">
                        <div className="epg-title">
                          {programme.title || '（无标题）'}
                          {results ? (
                            <span className="tag">
                              {names.get(programme.channel) ?? programme.channel}
                            </span>
                          ) : null}
                        </div>
                        {programme.desc ? (
                          <p className="muted epg-desc">{programme.desc}</p>
                        ) : null}
                      </div>
                    </li>
                  ))}
                </ul>
              )}
            </div>
          </section>
        </div>
      )}
    </div>
  );
}

createRoot(document.getElementById('root')!).render(<App />);
