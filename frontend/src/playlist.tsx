/** 订阅聚合：管理 `[playlist]` 源与过滤规则。 */

import { useCallback, useEffect, useState } from 'react';

import { api, copyText } from './api';
import { useToast } from './ui';

type Source = {
  id: string;
  name: string;
  url: string;
  enabled: boolean;
};

type PlaylistCfg = {
  merge_ttl_secs: number;
  merge_position: string;
  global_exclude: string[];
  sources: Source[];
};

const EMPTY: PlaylistCfg = {
  merge_ttl_secs: 300,
  merge_position: 'bottom',
  global_exclude: [],
  sources: [],
};

export function PlaylistPage() {
  const [cfg, setCfg] = useState<PlaylistCfg>(EMPTY);
  const [excludeText, setExcludeText] = useState('');
  const { node, ok, err } = useToast();

  const load = useCallback(async () => {
    try {
      const next = await api<PlaylistCfg>('GET', '/playlist');
      setCfg({ ...EMPTY, ...next, sources: next.sources ?? [] });
      setExcludeText((next.global_exclude ?? []).join('\n'));
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    try {
      const body: PlaylistCfg = {
        ...cfg,
        global_exclude: excludeText
          .split('\n')
          .map((line) => line.trim())
          .filter(Boolean),
      };
      await api('PUT', '/playlist', body);
      ok('聚合配置已保存');
      await load();
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  };

  const updateSource = (index: number, patch: Partial<Source>) => {
    const sources = cfg.sources.map((source, current) =>
      current === index ? { ...source, ...patch } : source,
    );
    setCfg({ ...cfg, sources });
  };

  return (
    <>
      <div className="card">
        <div className="row between">
          <div>
            <h1>订阅聚合</h1>
            <p className="muted">
              聚合清单：<code>{location.origin}/playlist.m3u</code>
            </p>
          </div>
          <div className="row">
            <button
              className="ghost"
              onClick={() =>
                void copyText(`${location.origin}/playlist.m3u`).then((done) =>
                  done ? ok('已复制聚合地址') : err('复制失败'),
                )
              }
            >
              复制地址
            </button>
            <button onClick={() => void save()}>保存</button>
          </div>
        </div>
        <div className="row">
          <label>
            缓存秒数
            <input
              type="number"
              value={cfg.merge_ttl_secs}
              onChange={(event) =>
                setCfg({ ...cfg, merge_ttl_secs: Number(event.target.value) || 300 })
              }
            />
          </label>
          <label>
            合并位置
            <select
              value={cfg.merge_position}
              onChange={(event) => setCfg({ ...cfg, merge_position: event.target.value })}
            >
              <option value="bottom">底部</option>
              <option value="top">顶部</option>
            </select>
          </label>
        </div>
      </div>

      <div className="card">
        <div className="row between">
          <h2>聚合源</h2>
          <button
            className="ghost"
            onClick={() =>
              setCfg({
                ...cfg,
                sources: [...cfg.sources, { id: '', name: '', url: '', enabled: true }],
              })
            }
          >
            添加源
          </button>
        </div>
        <table>
          <thead>
            <tr>
              <th>启用</th>
              <th>名称</th>
              <th>URL</th>
              <th>操作</th>
            </tr>
          </thead>
          <tbody>
            {cfg.sources.map((source, index) => (
              <tr key={source.id || index}>
                <td>
                  <input
                    type="checkbox"
                    checked={source.enabled}
                    onChange={(event) => updateSource(index, { enabled: event.target.checked })}
                  />
                </td>
                <td>
                  <input
                    value={source.name}
                    onChange={(event) => updateSource(index, { name: event.target.value })}
                  />
                </td>
                <td>
                  <input
                    value={source.url}
                    placeholder="http(s)://…/playlist.m3u"
                    onChange={(event) => updateSource(index, { url: event.target.value })}
                  />
                </td>
                <td>
                  <button
                    className="icon-btn"
                    onClick={() =>
                      setCfg({
                        ...cfg,
                        sources: cfg.sources.filter((_, current) => current !== index),
                      })
                    }
                  >
                    删除
                  </button>
                </td>
              </tr>
            ))}
            {cfg.sources.length === 0 && (
              <tr>
                <td colSpan={4} className="muted">
                  暂无聚合源。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      <div className="card">
        <h2>全局排除</h2>
        <p className="muted">每行一个关键词，匹配频道名。</p>
        <textarea
          value={excludeText}
          onChange={(event) => setExcludeText(event.target.value)}
          spellCheck={false}
        />
      </div>

      {node}
    </>
  );
}
