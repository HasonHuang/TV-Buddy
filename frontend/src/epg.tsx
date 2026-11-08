/** EPG 配置页：源、频道绑定、关键字规则。 */

import { useCallback, useEffect, useMemo, useState } from 'react';

import { api } from './api';
import { Badge, Confirm, Empty, Field, Modal, Select, Switch, Tip, useToast } from './ui';

type Header = { name: string; value: string };

type SourceUrl = {
  url: string;
  priority: number;
  user_agent: string;
  headers: Header[];
};

type Source = {
  id: string;
  name: string;
  enabled: boolean;
  kind: string;
  urls: SourceUrl[];
  refresh_secs: number;
  priority: number;
  script: string;
  output_files: string[];
  timeout_secs: number;
  tz_shift_mins: number;
  cron: string;
  last_fetch_started_at_ms?: number | null;
  last_fetch_finished_at_ms?: number | null;
  last_fetch_ok?: boolean;
  last_error?: string | null;
  channels?: number;
  programmes?: number;
  coverage_start_ms?: number | null;
  coverage_end_ms?: number | null;
  url_statuses?: UrlStatus[];
};

type UrlStatus = {
  url: string;
  priority: number;
  last_fetch_started_at_ms?: number | null;
  last_fetch_finished_at_ms?: number | null;
  last_fetch_ok: boolean;
  last_error?: string | null;
  channels: number;
  programmes: number;
};

type BindingRow = {
  gid: string;
  cid: string;
  name: string;
  group_id: string;
  group_name: string;
  out_id: string;
  source_id: string;
  source_name: string;
  raw_channel_id: string;
  upstream_display_name: string;
  origin: 'auto' | 'manual' | 'none' | string;
  updated_at_ms: number;
  match_via: string;
  programmes: number;
};

type SourceChannel = {
  id: string;
  display_names: string[];
  source_id: string;
  source_name: string;
};

type Condition = {
  op: 'all' | 'any' | 'not' | string;
  keywords: string[];
  groups: Condition[];
};

type Rule = {
  id: string;
  name: string;
  gid: string;
  cid: string;
  condition: Condition;
  action: 'record' | 'start' | 'stop' | string;
  lead_secs: number;
  tail_secs: number;
  output_dir: string;
  name_template: string;
  enabled: boolean;
  created_at_ms: number;
  updated_at_ms: number;
};

type RuleRow = {
  rule: Rule;
  group_name: string;
  channel_name: string;
};

type PreviewMatch = {
  title: string;
  start: string;
  stop: string;
  start_epoch: number;
  stop_epoch: number;
  disp_start: string;
  disp_end: string;
  duration_secs: number;
};

const ORIGIN_LABEL: Record<string, string> = {
  auto: '自动',
  manual: '手动',
  none: '未绑定',
};

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function timeAgo(ms?: number | null): string {
  if (!ms) return '—';
  const seconds = Math.max(0, Math.round((Date.now() - ms) / 1000));
  if (seconds < 60) return `${seconds} 秒前`;
  if (seconds < 3600) return `${Math.round(seconds / 60)} 分钟前`;
  if (seconds < 86400) return `${Math.round(seconds / 3600)} 小时前`;
  return `${Math.round(seconds / 86400)} 天前`;
}

function dateOf(ms?: number | null): string {
  if (!ms) return '—';
  return new Date(ms).toLocaleString();
}

function headersText(headers: Header[]): string {
  return headers.map((header) => `${header.name}: ${header.value}`).join('\n');
}

function parseHeaders(text: string): Header[] {
  return text
    .split('\n')
    .map((line) => {
      const index = line.indexOf(':');
      if (index < 0) return null;
      const name = line.slice(0, index).trim();
      const value = line.slice(index + 1).trim();
      return name && value ? { name, value } : null;
    })
    .filter((item): item is Header => item !== null);
}

function toSourcePayload(source: Source): Source {
  return {
    id: source.id,
    name: source.name,
    enabled: source.enabled,
    kind: source.kind,
    urls: source.urls.map((url) => ({
      url: url.url,
      priority: Number(url.priority) || 0,
      user_agent: url.user_agent,
      headers: url.headers,
    })),
    refresh_secs: Number(source.refresh_secs) || 0,
    priority: Number(source.priority) || 0,
    script: source.script,
    output_files: source.output_files,
    timeout_secs: Number(source.timeout_secs) || 0,
    tz_shift_mins: Number(source.tz_shift_mins) || 0,
    cron: source.cron,
  };
}

/* ---------------- 源 ---------------- */

type UrlForm = {
  url: string;
  priority: string;
  user_agent: string;
  headers: string;
};

type SourceForm = {
  id: string;
  name: string;
  enabled: boolean;
  kind: string;
  urls: UrlForm[];
  refresh_mins: string;
  priority: string;
  script: string;
  output_files: string;
  timeout_secs: string;
  tz_shift_mins: string;
  cron: string;
};

function emptyUrl(): UrlForm {
  return { url: '', priority: '0', user_agent: '', headers: '' };
}

function formOf(source?: Source): SourceForm {
  if (!source) {
    return {
      id: '',
      name: '',
      enabled: true,
      kind: 'xml',
      urls: [emptyUrl()],
      refresh_mins: '360',
      priority: '0',
      script: '',
      output_files: '',
      timeout_secs: '0',
      tz_shift_mins: '0',
      cron: '',
    };
  }
  return {
    id: source.id,
    name: source.name,
    enabled: source.enabled,
    kind: source.kind || 'xml',
    urls:
      source.urls.length > 0
        ? source.urls.map((url) => ({
            url: url.url,
            priority: String(url.priority ?? 0),
            user_agent: url.user_agent ?? '',
            headers: headersText(url.headers ?? []),
          }))
        : [emptyUrl()],
    refresh_mins: String(Math.round((source.refresh_secs || 0) / 60)),
    priority: String(source.priority ?? 0),
    script: source.script ?? '',
    output_files: (source.output_files ?? []).join(', '),
    timeout_secs: String(source.timeout_secs ?? 0),
    tz_shift_mins: String(source.tz_shift_mins ?? 0),
    cron: source.cron ?? '',
  };
}

function payloadOf(form: SourceForm): Source {
  return {
    id: form.id,
    name: form.name,
    enabled: form.enabled,
    kind: form.kind,
    urls: form.urls
      .filter((url) => url.url.trim() !== '')
      .map((url) => ({
        url: url.url.trim(),
        priority: Number(url.priority) || 0,
        user_agent: url.user_agent.trim(),
        headers: parseHeaders(url.headers),
      })),
    refresh_secs: Math.max(0, Math.round((Number(form.refresh_mins) || 0) * 60)),
    priority: Number(form.priority) || 0,
    script: form.script.trim(),
    output_files: form.output_files
      .split(',')
      .map((item) => item.trim())
      .filter(Boolean),
    timeout_secs: Math.max(0, Number(form.timeout_secs) || 0),
    tz_shift_mins: Number(form.tz_shift_mins) || 0,
    cron: form.cron.trim(),
  };
}

function SourceModal({
  source,
  onClose,
  onSaved,
}: {
  source?: Source;
  onClose: () => void;
  onSaved: () => void;
}) {
  const [form, setForm] = useState<SourceForm>(() => formOf(source));
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const set = (key: keyof SourceForm, value: string | boolean) => {
    setForm((current) => ({ ...current, [key]: value }));
  };

  const setUrl = (index: number, key: keyof UrlForm, value: string) => {
    setForm((current) => ({
      ...current,
      urls: current.urls.map((url, at) => (at === index ? { ...url, [key]: value } : url)),
    }));
  };

  const save = async () => {
    setBusy(true);
    setError('');
    try {
      await api('POST', '/epg/sources', payloadOf(form));
      onSaved();
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={source ? '编辑 EPG 源' : '添加 EPG 源'}
      onClose={onClose}
      footer={
        <>
          <button className="ghost" onClick={onClose} disabled={busy}>
            取消
          </button>
          <button onClick={() => void save()} disabled={busy || form.name.trim() === ''}>
            {busy ? '保存中…' : '保存'}
          </button>
        </>
      }
    >
      <div className="fields">
        <Field label="名称" tip="面板里显示的名字。">
          <input
            value={form.name}
            onChange={(event) => set('name', event.target.value)}
            placeholder="例如 主源"
          />
        </Field>
        <Field label="类型" tip="XMLTV 源抓 URL；脚本源运行 Python 脚本。">
          <Select
            value={form.kind}
            options={[
              { value: 'xml', label: 'XMLTV 源' },
              { value: 'script', label: '脚本源' },
            ]}
            onChange={(next) => set('kind', next)}
          />
        </Field>
      </div>

      {form.kind === 'xml' ? (
        <div className="source-urls">
          <div className="row between">
            <strong>URL 列表</strong>
            <button
              className="ghost"
              onClick={() => setForm((current) => ({ ...current, urls: [...current.urls, emptyUrl()] }))}
            >
              添加 URL
            </button>
          </div>
          <p className="muted">
            同一个源可挂多个 URL。多个 URL 含同一频道时，优先级数字小的 URL 获胜。
          </p>
          {form.urls.map((url, index) => (
            <div className="source-url" key={index}>
              <div className="row">
                <input
                  aria-label="URL 优先级"
                  title="数字越小越优先"
                  type="number"
                  style={{ width: 76 }}
                  value={url.priority}
                  onChange={(event) => setUrl(index, 'priority', event.target.value)}
                />
                <input
                  aria-label="EPG URL"
                  placeholder="https://example.com/epg.xml.gz"
                  value={url.url}
                  onChange={(event) => setUrl(index, 'url', event.target.value)}
                />
                <button
                  className="icon-btn danger-text"
                  title="删除这个 URL"
                  onClick={() =>
                    setForm((current) => ({
                      ...current,
                      urls:
                        current.urls.length > 1
                          ? current.urls.filter((_, at) => at !== index)
                          : [emptyUrl()],
                    }))
                  }
                >
                  删除
                </button>
              </div>
              <input
                aria-label="User-Agent"
                placeholder="User-Agent（留空用默认）"
                value={url.user_agent}
                onChange={(event) => setUrl(index, 'user_agent', event.target.value)}
              />
              <textarea
                aria-label="自定义请求头"
                placeholder={'自定义请求头，每行 Header: Value\nReferer: https://example.com/'}
                rows={2}
                value={url.headers}
                onChange={(event) => setUrl(index, 'headers', event.target.value)}
              />
            </div>
          ))}
        </div>
      ) : (
        <div className="fields">
          <Field label="脚本文件名" tip="位于缓存目录 epg/scripts 下的 .py 文件。">
            <input
              value={form.script}
              onChange={(event) => set('script', event.target.value)}
              placeholder="mysource.py"
            />
          </Field>
          <Field label="产物文件" tip="脚本没有标准输出时，按顺序读取这些文件。">
            <input
              value={form.output_files}
              onChange={(event) => set('output_files', event.target.value)}
              placeholder="epg.xml, epg2.xml"
            />
          </Field>
          <Field label="超时（秒）" tip="0 表示使用全局脚本超时。">
            <input
              type="number"
              value={form.timeout_secs}
              onChange={(event) => set('timeout_secs', event.target.value)}
            />
          </Field>
        </div>
      )}

      <div className="fields">
        <Field label="刷新间隔（分钟）" tip="0 表示沿用 cron 或全局采集节奏。">
          <input
            type="number"
            value={form.refresh_mins}
            onChange={(event) => set('refresh_mins', event.target.value)}
          />
        </Field>
        <Field label="源优先级" tip="数字越小越优先。多源同频道时高优先级覆盖低优先级。">
          <input
            type="number"
            value={form.priority}
            onChange={(event) => set('priority', event.target.value)}
          />
        </Field>
        <Field label="时区偏移（分钟）" tip="节目时间整体平移。0 表示不改。">
          <input
            type="number"
            value={form.tz_shift_mins}
            onChange={(event) => set('tz_shift_mins', event.target.value)}
          />
        </Field>
        <Field label="旧 cron" tip="仅旧配置使用。非空时按 cron 调度。">
          <input
            value={form.cron}
            onChange={(event) => set('cron', event.target.value)}
            placeholder="留空不用"
          />
        </Field>
      </div>

      <label className="row">
        <Switch checked={form.enabled} onChange={(next) => set('enabled', next)} label="启用源" />
        <span>启用</span>
      </label>
      {error && <div className="msg err">{error}</div>}
    </Modal>
  );
}

export function EpgSourcesTab() {
  const [sources, setSources] = useState<Source[]>([]);
  const [editing, setEditing] = useState<Source | undefined>();
  const [adding, setAdding] = useState(false);
  const [pendingDelete, setPendingDelete] = useState<Source | null>(null);
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState('');
  const toast = useToast();

  const load = useCallback(async () => {
    try {
      setSources(await api<Source[]>('GET', '/epg/sources'));
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async (source: Source) => {
    setBusy(source.id);
    try {
      await api('POST', '/epg/sources', toSourcePayload(source));
      await load();
      toast.ok('EPG 源已保存');
    } catch (err) {
      toast.err(errorText(err));
    } finally {
      setBusy('');
    }
  };

  const remove = async () => {
    if (!pendingDelete) return;
    try {
      await api('DELETE', `/epg/sources/${encodeURIComponent(pendingDelete.id)}`);
      setPendingDelete(null);
      setMessage('EPG 源已删除');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const refresh = async (source: Source) => {
    setBusy(source.id);
    setMessage('');
    setError('');
    try {
      const result = await api<{ channels: number; programmes: number }>(
        'POST',
        `/epg/sources/${encodeURIComponent(source.id)}/refresh`,
      );
      setMessage(`「${source.name}」采集完成：${result.channels} 个频道，${result.programmes} 条节目`);
      await load();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy('');
    }
  };

  return (
    <div className="card">
      <div className="card-head">
        <div className="card-head-text">
          <h2>
            EPG 源
            <Tip text="配置 XMLTV 源。支持 .xml / .xml.gz，自动 ETag 减少上游负载。" />
          </h2>
          <span className="muted">按优先级合并。同一频道由高优先级源提供节目。</span>
        </div>
        <div className="card-actions">
          <button onClick={() => setAdding(true)} title="添加一个 XMLTV 源">
            添加源
          </button>
        </div>
      </div>
      {message && <div className="msg ok">{message}</div>}
      {error && <div className="msg err">{error}</div>}
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>名称</th>
              <th>地址</th>
              <th>优先级</th>
              <th>间隔</th>
              <th>状态</th>
              <th>覆盖</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {sources.map((source) => (
              <tr key={source.id}>
                <td>
                  <div className="row">
                    <Switch
                      checked={source.enabled}
                      onChange={(next) => void save({ ...source, enabled: next })}
                      disabled={busy === source.id}
                      label={`启用 ${source.name}`}
                    />
                    <strong>{source.name}</strong>
                  </div>
                  <div className="muted mono">{source.id.slice(0, 8)}</div>
                </td>
                <td>
                  <div className="source-url-list">
                    {(source.urls ?? []).map((url) => {
                      const status = source.url_statuses?.find((item) => item.url === url.url);
                      return (
                        <div key={`${url.url}-${url.priority}`}>
                          <div className="mono ellipsis" title={url.url}>
                            {url.url || '—'}
                          </div>
                          {status && (
                            <div className={status.last_fetch_ok ? 'muted' : 'danger-text'}>
                              {status.last_fetch_ok ? '正常' : '失败'} · {timeAgo(status.last_fetch_finished_at_ms)}
                              {status.last_error ? ` · ${status.last_error}` : ''}
                            </div>
                          )}
                        </div>
                      );
                    })}
                    {(source.urls ?? []).length > 1 && (
                      <div className="muted">{source.urls.length} 个 URL</div>
                    )}
                    {source.kind === 'script' && (
                      <div className="muted">脚本：{source.script || '—'}</div>
                    )}
                  </div>
                </td>
                <td>{source.priority ?? 0}</td>
                <td>{source.refresh_secs > 0 ? `${Math.round(source.refresh_secs / 60)}m` : source.cron || '全局'}</td>
                <td>
                  {source.last_fetch_started_at_ms ? (
                    <div>
                      <Badge tone={source.last_fetch_ok ? 'ok' : 'danger'}>
                        {source.last_fetch_ok ? '正常' : '失败'}
                      </Badge>
                      <div className="muted">{timeAgo(source.last_fetch_finished_at_ms)}</div>
                    </div>
                  ) : (
                    <Badge tone="muted">未采集</Badge>
                  )}
                  {source.last_error && (
                    <div className="muted ellipsis" title={source.last_error}>
                      {source.last_error}
                    </div>
                  )}
                </td>
                <td>
                  <div>
                    {source.channels ?? 0} ch · {source.programmes ?? 0} prog
                  </div>
                  <div className="muted">
                    覆盖至 {source.coverage_end_ms ? dateOf(source.coverage_end_ms) : '—'}
                  </div>
                </td>
                <td className="right">
                  <div className="row" style={{ justifyContent: 'flex-end' }}>
                    <button
                      className="ghost"
                      title="立即采集这个源"
                      disabled={busy === source.id}
                      onClick={() => void refresh(source)}
                    >
                      {busy === source.id ? '采集中…' : '采集'}
                    </button>
                    <button className="ghost" title="编辑这个源" onClick={() => setEditing(source)}>
                      编辑
                    </button>
                    <button
                      className="ghost danger-text"
                      title="删除这个源及其绑定"
                      onClick={() => setPendingDelete(source)}
                    >
                      删除
                    </button>
                  </div>
                </td>
              </tr>
            ))}
            {sources.length === 0 && (
              <tr>
                <td colSpan={7} className="muted">
                  还没有 EPG 源。先添加一个 XMLTV 地址。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {toast.node}
      {(adding || editing) && (
        <SourceModal
          source={editing}
          onClose={() => {
            setAdding(false);
            setEditing(undefined);
          }}
          onSaved={() => void load()}
        />
      )}
      {pendingDelete && (
        <Confirm
          title="删除 EPG 源"
          text={`删除「${pendingDelete.name}」？该源的所有频道绑定会一起删除。`}
          onCancel={() => setPendingDelete(null)}
          onConfirm={() => void remove()}
        />
      )}
    </div>
  );
}

/* ---------------- 绑定 ---------------- */

function BindingModal({
  row,
  sources,
  boundRows,
  onClose,
  onChanged,
}: {
  row: BindingRow;
  sources: Source[];
  boundRows: BindingRow[];
  onClose: () => void;
  onChanged: () => void;
}) {
  const [mode, setMode] = useState<'source' | 'copy'>('source');
  const [sourceId, setSourceId] = useState(sources[0]?.id ?? '');
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<SourceChannel[]>([]);
  const [copyFrom, setCopyFrom] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    if (mode !== 'source' || !sourceId) return;
    let alive = true;
    setLoading(true);
    const timer = window.setTimeout(() => {
      api<{ items: SourceChannel[] }>(
        'GET',
        `/epg/source-channels/${encodeURIComponent(sourceId)}?q=${encodeURIComponent(query)}&limit=200`,
      )
        .then((data) => {
          if (alive) setItems(data.items ?? []);
        })
        .catch((err) => {
          if (alive) setError(errorText(err));
        })
        .finally(() => {
          if (alive) setLoading(false);
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [mode, sourceId, query]);

  const bind = async (rawChannelId: string) => {
    setBusy(true);
    setError('');
    try {
      await api('PUT', `/epg/bindings/${encodeURIComponent(row.gid)}/${encodeURIComponent(row.cid)}`, {
        source_id: sourceId,
        raw_channel_id: rawChannelId,
      });
      onChanged();
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    if (!copyFrom) return;
    const [fromGid, fromCid] = JSON.parse(copyFrom) as [string, string];
    setBusy(true);
    setError('');
    try {
      await api(
        'POST',
        `/epg/bindings/${encodeURIComponent(row.gid)}/${encodeURIComponent(row.cid)}/copy`,
        { from_gid: fromGid, from_cid: fromCid },
      );
      onChanged();
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const clear = async () => {
    setBusy(true);
    setError('');
    try {
      await api(
        'DELETE',
        `/epg/bindings/${encodeURIComponent(row.gid)}/${encodeURIComponent(row.cid)}`,
      );
      onChanged();
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={`为「${row.name}」选择 EPG`}
      onClose={onClose}
      footer={
        <>
          {row.origin !== 'none' && (
            <button className="ghost danger-text" onClick={() => void clear()} disabled={busy}>
              清除绑定
            </button>
          )}
          <button className="ghost" onClick={onClose} disabled={busy}>
            关闭
          </button>
          {mode === 'copy' && (
            <button onClick={() => void copy()} disabled={busy || !copyFrom}>
              {busy ? '处理中…' : '复用'}
            </button>
          )}
        </>
      }
    >
      <div className="row" style={{ marginBottom: 12 }}>
        <button
          className={mode === 'source' ? '' : 'ghost'}
          onClick={() => setMode('source')}
        >
          绑定到 EPG 源
        </button>
        <button className={mode === 'copy' ? '' : 'ghost'} onClick={() => setMode('copy')}>
          复用其他频道的 EPG
        </button>
      </div>
      {mode === 'source' ? (
        <>
          <div className="fields">
            <Field label="EPG 源" tip="选择要绑定的源。">
              <Select
                value={sourceId}
                options={sources.map((source) => ({ value: source.id, label: source.name }))}
                onChange={setSourceId}
                placeholder="选择源"
              />
            </Field>
          </div>
          <div className="row" style={{ marginBottom: 8 }}>
            <input
              aria-label="搜索源频道"
              placeholder="搜索源频道名称或 ID"
              value={query}
              onChange={(event) => setQuery(event.target.value)}
            />
            <span className="muted">{loading ? '加载中…' : `${items.length} 个频道`}</span>
          </div>
          <div className="binding-list">
            {items.map((item) => (
              <button
                className="binding-item"
                key={`${item.source_id}-${item.id}`}
                disabled={busy}
                onClick={() => void bind(item.id)}
                title={`绑定到 ${item.id}`}
              >
                <span className="mono">{item.id}</span>
                <span>{item.display_names.join(' / ')}</span>
              </button>
            ))}
            {!loading && items.length === 0 && <Empty text="没有匹配的源频道" icon="search" />}
          </div>
        </>
      ) : (
        <div className="fields">
          <Field label="复用来源" tip="复制另一个频道的源与源频道 ID。">
            <Select
              value={copyFrom}
              options={boundRows
                .filter((item) => item.origin !== 'none' && !(item.gid === row.gid && item.cid === row.cid))
                .map((item) => ({
                  value: JSON.stringify([item.gid, item.cid]),
                  label: `${item.name}（${item.source_name} / ${item.raw_channel_id}）`,
                }))}
              onChange={setCopyFrom}
              placeholder="选择已绑定的频道"
            />
          </Field>
        </div>
      )}
      {error && <div className="msg err">{error}</div>}
    </Modal>
  );
}

export function EpgBindingsTab() {
  const [rows, setRows] = useState<BindingRow[]>([]);
  const [sources, setSources] = useState<Source[]>([]);
  const [group, setGroup] = useState('');
  const [query, setQuery] = useState('');
  const [origin, setOrigin] = useState('all');
  const [editing, setEditing] = useState<BindingRow | null>(null);
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const [data, sourceList] = await Promise.all([
        api<{ items: BindingRow[]; bound: number; total: number }>('GET', '/epg/bindings'),
        api<Source[]>('GET', '/epg/sources'),
      ]);
      setRows(data.items ?? []);
      setSources(sourceList);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const groups = useMemo(() => {
    const map = new Map<string, string>();
    for (const row of rows) map.set(row.group_id, row.group_name);
    return [...map.entries()].map(([value, label]) => ({ value, label }));
  }, [rows]);

  const visible = rows.filter((row) => {
    if (group && row.group_id !== group) return false;
    if (origin === 'unbound' && row.origin !== 'none') return false;
    if (origin === 'auto' && row.origin !== 'auto') return false;
    if (origin === 'manual' && row.origin !== 'manual') return false;
    const text = query.trim().toLowerCase();
    if (text && !row.name.toLowerCase().includes(text) && !row.out_id.toLowerCase().includes(text)) {
      return false;
    }
    return true;
  });
  const bound = rows.filter((row) => row.origin !== 'none').length;

  return (
    <div className="card">
      <div className="card-head">
        <div className="card-head-text">
          <h2>
            EPG 绑定
            <Tip text="源刷新后自动按精确匹配写入绑定。未匹配的频道在这里手动指定。" />
          </h2>
          <span className="muted">
            已绑定 {bound}/{rows.length} · {rows.length - bound} 个未绑定
          </span>
        </div>
      </div>
      <div className="row" style={{ marginBottom: 10 }}>
        <Select
          value={group}
          options={[{ value: '', label: '全部分组' }, ...groups]}
          onChange={setGroup}
          width={160}
        />
        <input
          aria-label="搜索频道"
          placeholder="搜索频道名称或 ID"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
        />
        <Select
          value={origin}
          options={[
            { value: 'all', label: '全部' },
            { value: 'unbound', label: '未绑定' },
            { value: 'auto', label: '自动' },
            { value: 'manual', label: '手动' },
          ]}
          onChange={setOrigin}
          width={110}
        />
      </div>
      {error && <div className="msg err">{error}</div>}
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>推流频道</th>
              <th>绑定</th>
              <th>上游显示名</th>
              <th>来源</th>
              <th>节目</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {visible.map((row) => (
              <tr key={`${row.gid}/${row.cid}`}>
                <td>
                  <div>
                    <strong>{row.name}</strong>
                    <span className="muted"> · {row.group_name}</span>
                  </div>
                  <div className="muted mono">{row.gid}/{row.cid}</div>
                </td>
                <td>
                  <Badge tone={row.origin === 'manual' ? 'ok' : row.origin === 'auto' ? 'info' : 'warn'}>
                    {ORIGIN_LABEL[row.origin] ?? row.origin}
                  </Badge>
                  {row.raw_channel_id && <span className="mono"> {row.raw_channel_id}</span>}
                </td>
                <td>{row.upstream_display_name || '—'}</td>
                <td>{row.source_name || '—'}</td>
                <td>{row.programmes || 0}</td>
                <td className="right">
                  <button className="ghost" title="选择 EPG 频道" onClick={() => setEditing(row)}>
                    选择
                  </button>
                </td>
              </tr>
            ))}
            {visible.length === 0 && (
              <tr>
                <td colSpan={6} className="muted">
                  没有匹配的频道。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {editing && (
        <BindingModal
          row={editing}
          sources={sources}
          boundRows={rows}
          onClose={() => setEditing(null)}
          onChanged={() => void load()}
        />
      )}
    </div>
  );
}

/* ---------------- 规则 ---------------- */

function emptyCondition(): Condition {
  return { op: 'all', keywords: [], groups: [] };
}

function ConditionEditor({
  value,
  onChange,
  depth = 0,
}: {
  value: Condition;
  onChange: (next: Condition) => void;
  depth?: number;
}) {
  const set = (patch: Partial<Condition>) => onChange({ ...value, ...patch });
  return (
    <div className="condition-node">
      <div className="row">
        <Select
          value={value.op || 'all'}
          options={[
            { value: 'all', label: '全部满足' },
            { value: 'any', label: '任一满足' },
            { value: 'not', label: '全部不满足' },
          ]}
          onChange={(next) => set({ op: next })}
          width={140}
        />
        <input
          aria-label="关键字"
          placeholder="关键字，用逗号分隔"
          value={value.keywords.join(', ')}
          onChange={(event) =>
            set({
              keywords: event.target.value
                .split(',')
                .map((item) => item.trim())
                .filter(Boolean),
            })
          }
        />
        {depth < 3 && (
          <button
            className="ghost"
            title="添加子条件组"
            onClick={() => set({ groups: [...value.groups, emptyCondition()] })}
          >
            加分组
          </button>
        )}
      </div>
      {value.groups.map((group, index) => (
        <div className="condition-group" key={index}>
          <div className="row between">
            <span className="muted">子组 {index + 1}</span>
            <button
              className="icon-btn danger-text"
              title="删除这个子组"
              onClick={() => set({ groups: value.groups.filter((_, at) => at !== index) })}
            >
              删除
            </button>
          </div>
          <ConditionEditor
            value={group}
            depth={depth + 1}
            onChange={(next) =>
              set({ groups: value.groups.map((item, at) => (at === index ? next : item)) })
            }
          />
        </div>
      ))}
    </div>
  );
}

function conditionSummary(condition: Condition): string {
  const parts: string[] = [];
  if (condition.keywords.length > 0) parts.push(condition.keywords.join('、'));
  for (const group of condition.groups) {
    const inner = conditionSummary(group);
    if (inner) parts.push(`(${inner})`);
  }
  if (parts.length === 0) return '空';
  const joiner = condition.op === 'any' ? ' 或 ' : condition.op === 'not' ? ' 且非 ' : ' 且 ';
  return parts.join(joiner);
}

type RuleDraft = {
  id: string;
  name: string;
  key: string;
  condition: Condition;
  action: string;
  lead_secs: string;
  tail_secs: string;
  output_dir: string;
  name_template: string;
  enabled: boolean;
  created_at_ms: number;
};

function ruleDraft(rule?: Rule, key = ''): RuleDraft {
  if (!rule) {
    return {
      id: '',
      name: '',
      key,
      condition: emptyCondition(),
      action: 'record',
      lead_secs: '0',
      tail_secs: '0',
      output_dir: '',
      name_template: '{program.title} {datetime}',
      enabled: true,
      created_at_ms: 0,
    };
  }
  return {
    id: rule.id,
    name: rule.name,
    key: JSON.stringify([rule.gid, rule.cid]),
    condition: rule.condition ?? emptyCondition(),
    action: rule.action || 'record',
    lead_secs: String(rule.lead_secs ?? 0),
    tail_secs: String(rule.tail_secs ?? 0),
    output_dir: rule.output_dir ?? '',
    name_template: rule.name_template || '{program.title} {datetime}',
    enabled: rule.enabled,
    created_at_ms: rule.created_at_ms ?? 0,
  };
}

function RuleModal({
  rule,
  rows,
  onClose,
  onSaved,
}: {
  rule?: Rule;
  rows: BindingRow[];
  onClose: () => void;
  onSaved: () => void;
}) {
  const [draft, setDraft] = useState<RuleDraft>(() => ruleDraft(rule));
  const [preview, setPreview] = useState<{
    bound: boolean;
    matches: PreviewMatch[];
    lookahead_secs?: number;
    note?: string;
  } | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState('');

  const set = (patch: Partial<RuleDraft>) => setDraft((current) => ({ ...current, ...patch }));

  const payload = (): Rule => {
    const [gid, cid] = draft.key ? (JSON.parse(draft.key) as [string, string]) : ['', ''];
    return {
      id: draft.id,
      name: draft.name,
      gid: gid ?? '',
      cid: cid ?? '',
      condition: draft.condition,
      action: draft.action,
      lead_secs: Number(draft.lead_secs) || 0,
      tail_secs: Number(draft.tail_secs) || 0,
      output_dir: draft.output_dir,
      name_template: draft.name_template,
      enabled: draft.enabled,
      created_at_ms: draft.created_at_ms,
      updated_at_ms: 0,
    };
  };

  const runPreview = async () => {
    setBusy(true);
    setError('');
    try {
      const result = await api<{
        bound: boolean;
        matches: PreviewMatch[];
        lookahead_secs?: number;
      }>('POST', '/epg/rules/preview', payload());
      setPreview({
        bound: result.bound,
        matches: result.matches ?? [],
        lookahead_secs: result.lookahead_secs,
      });
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    setBusy(true);
    setError('');
    try {
      await api('POST', '/epg/rules', payload());
      onSaved();
      onClose();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal
      title={rule ? '编辑关键字规则' : '新建关键字规则'}
      onClose={onClose}
      footer={
        <>
          <button className="ghost" onClick={onClose} disabled={busy}>
            取消
          </button>
          <button className="ghost" onClick={() => void runPreview()} disabled={busy || !draft.key}>
            预览匹配
          </button>
          <button
            onClick={() => void save()}
            disabled={busy || draft.name.trim() === '' || !draft.key}
          >
            {busy ? '处理中…' : '保存'}
          </button>
        </>
      }
    >
      <div className="fields">
        <Field label="名称" tip="规则的显示名称。">
          <input
            value={draft.name}
            onChange={(event) => set({ name: event.target.value })}
            placeholder="例如 利物浦比赛自动录制"
          />
        </Field>
        <Field label="频道" tip="只扫描这个频道的节目。">
          <Select
            value={draft.key}
            options={rows.map((row) => ({
              value: JSON.stringify([row.gid, row.cid]),
              label: `${row.name}（${row.group_name}）`,
              desc: row.origin === 'none' ? '未绑定 EPG' : row.source_name,
            }))}
            onChange={(next) => set({ key: next })}
            placeholder="选择一个频道"
          />
        </Field>
      </div>

      <Field
        label="匹配条件"
        tip="对节目标题做大小写不敏感的子串匹配。规则只扫描生成节目单的未来窗口。"
      >
        <ConditionEditor value={draft.condition} onChange={(next) => set({ condition: next })} />
      </Field>

      <div className="fields">
        <Field label="动作" tip="录制会创建定时任务；开始与停止切换回看缓冲。">
          <Select
            value={draft.action}
            options={[
              { value: 'record', label: '录制' },
              { value: 'start', label: '开始' },
              { value: 'stop', label: '停止' },
            ]}
            onChange={(next) => set({ action: next })}
          />
        </Field>
        <Field label="提前（秒）" tip="录制在节目开始前多久开始。">
          <input
            type="number"
            value={draft.lead_secs}
            onChange={(event) => set({ lead_secs: event.target.value })}
          />
        </Field>
        <Field label="延长（秒）" tip="录制在节目结束后继续多久。">
          <input
            type="number"
            value={draft.tail_secs}
            onChange={(event) => set({ tail_secs: event.target.value })}
          />
        </Field>
      </div>
      <Field label="输出目录" tip="相对缓存目录 recordings 的子目录。留空用默认。">
        <input
          value={draft.output_dir}
          onChange={(event) => set({ output_dir: event.target.value })}
          placeholder="留空使用全局默认"
        />
      </Field>
      <Field
        label="任务名模板"
        tip="占位符：{program.title} {channel.name} {date} {time} {datetime}；时间可加偏移，如 {datetime+8}。用 / 分隔可建子目录。"
      >
        <input
          value={draft.name_template}
          onChange={(event) => set({ name_template: event.target.value })}
          placeholder="{program.title} {datetime}"
        />
      </Field>
      <label className="row">
        <Switch checked={draft.enabled} onChange={(next) => set({ enabled: next })} label="启用规则" />
        <span>启用</span>
      </label>

      {preview && (
        <div className="rule-preview">
          <div className="row between">
            <strong>
              未来 {Math.round((preview.lookahead_secs ?? 0) / 3600)} 小时命中：
              {preview.matches.length} 条
            </strong>
            <button className="ghost" onClick={() => setPreview(null)}>
              收起
            </button>
          </div>
          {!preview.bound && (
            <div className="msg err">该频道还没有 EPG 绑定，规则不会命中。</div>
          )}
          {preview.matches.length > 0 && (
            <div className="table-wrap">
              <table className="table">
                <thead>
                  <tr>
                    <th>标题</th>
                    <th>开始</th>
                    <th>时长</th>
                  </tr>
                </thead>
                <tbody>
                  {preview.matches.map((item) => (
                    <tr key={`${item.start}-${item.title}`}>
                      <td>{item.title}</td>
                      <td className="mono">{item.disp_start}</td>
                      <td>{Math.round(item.duration_secs / 60)} 分钟</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </div>
      )}
      {error && <div className="msg err">{error}</div>}
    </Modal>
  );
}

export function EpgRulesTab() {
  const [rows, setRows] = useState<RuleRow[]>([]);
  const [bindings, setBindings] = useState<BindingRow[]>([]);
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<Rule | undefined>();
  const [pendingDelete, setPendingDelete] = useState<Rule | null>(null);
  const [error, setError] = useState('');
  const toast = useToast();

  const load = useCallback(async () => {
    try {
      const [ruleRows, bindingData] = await Promise.all([
        api<RuleRow[]>('GET', '/epg/rules'),
        api<{ items: BindingRow[] }>('GET', '/epg/bindings'),
      ]);
      setRows(ruleRows);
      setBindings(bindingData.items ?? []);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const toggle = async (rule: Rule, enabled: boolean) => {
    try {
      await api('POST', '/epg/rules', { ...rule, enabled });
      await load();
    } catch (err) {
      toast.err(errorText(err));
    }
  };

  const remove = async () => {
    if (!pendingDelete) return;
    try {
      const result = await api<{ removed_tasks: number }>(
        'DELETE',
        `/epg/rules/${encodeURIComponent(pendingDelete.id)}`,
      );
      setPendingDelete(null);
      toast.ok(`规则已删除，同步移除 ${result.removed_tasks} 个待执行任务`);
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  return (
    <div className="card">
      <div className="card-head">
        <div className="card-head-text">
          <h2>
            关键字规则
            <Tip text="节目标题命中关键字时自动开始、停止或录制。" />
          </h2>
          <span className="muted">条件可用全部满足 / 任一满足 / 全部不满足嵌套。</span>
        </div>
        <div className="card-actions">
          <button onClick={() => setAdding(true)} title="新建一条关键字规则">
            新建规则
          </button>
        </div>
      </div>
      {error && <div className="msg err">{error}</div>}
      <div className="table-wrap">
        <table className="table">
          <thead>
            <tr>
              <th>名称</th>
              <th>频道</th>
              <th>条件</th>
              <th>动作</th>
              <th>启用</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {rows.map(({ rule, group_name, channel_name }) => (
              <tr key={rule.id}>
                <td>{rule.name}</td>
                <td>
                  {channel_name}
                  <span className="muted"> · {group_name}</span>
                </td>
                <td className="ellipsis" title={conditionSummary(rule.condition)}>
                  {conditionSummary(rule.condition)}
                </td>
                <td>{rule.action === 'record' ? '录制' : rule.action === 'start' ? '开始' : '停止'}</td>
                <td>
                  <Switch
                    checked={rule.enabled}
                    onChange={(next) => void toggle(rule, next)}
                    label={`启用 ${rule.name}`}
                  />
                </td>
                <td className="right">
                  <div className="row" style={{ justifyContent: 'flex-end' }}>
                    <button className="ghost" onClick={() => setEditing(rule)}>
                      编辑
                    </button>
                    <button
                      className="ghost danger-text"
                      onClick={() => setPendingDelete(rule)}
                      title="删除规则与它生成的待执行任务"
                    >
                      删除
                    </button>
                  </div>
                </td>
              </tr>
            ))}
            {rows.length === 0 && (
              <tr>
                <td colSpan={6} className="muted">
                  还没有关键字规则。例如：标题含「直播」且（「利物浦」或「曼联」）→ 自动录制。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
      {toast.node}
      {(adding || editing) && (
        <RuleModal
          rule={editing}
          rows={bindings}
          onClose={() => {
            setAdding(false);
            setEditing(undefined);
          }}
          onSaved={() => void load()}
        />
      )}
      {pendingDelete && (
        <Confirm
          title="删除关键字规则"
          text={`删除规则「${pendingDelete.name}」？它生成的待执行任务会一起删除。`}
          onCancel={() => setPendingDelete(null)}
          onConfirm={() => void remove()}
        />
      )}
    </div>
  );
}
