/**
 * 运维分区：安全管控、运行日志、备份与恢复。
 *
 * 接口：`/panel/api/security/*`、`/panel/api/logs`、`/panel/api/{backup,restore}`。
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import { api, apiText, getToken } from './api';
import { Icon } from './icons';
import { PageHead } from './ui';

type TokenInfo = {
  enabled: boolean;
  ttl_hours: number;
  /** 目标字段：明文 Token。 */
  current_value?: string;
  status?: {
    current?: { remaining_secs: number; valid: boolean; value: string };
    old?: { remaining_secs: number; valid: boolean; value: string } | null;
    ttl_hours: number;
  };
  /** 本实现附加字段（旧字段）。 */
  current: string;
  old: string | null;
};

type IpEntry = {
  ip: string;
  requests: number;
  failures: number;
  banned: boolean;
  remaining_secs: number;
};

type IpStats = {
  count: number;
  banned: number;
  entries: IpEntry[];
};

/**
 * `[auth]` 段的可编辑视图（A3）。
 *
 * 后端没有单独的 auth 写接口，所以与设置页同一机制：`GET /panel/api/config`
 * 取整份 → 改 `auth` 子树 → `PUT /panel/api/config` 全量回写（S-CFG-01）。
 *
 * `[auth]` 的全部键都已生效。并发会话、会话超时、白名单、限流与升级封禁
 * 只在 `ip_guard_enabled` 打开时参与判定；其余键始终生效。
 */
type AuthCfg = Record<string, unknown>;
type FullConfig = { auth?: AuthCfg } & Record<string, unknown>;

/** 数字字段：路径 + 标签 + 单位。 */
type NumField = { key: string; label: string; hint?: string };

const AUTH_LIMIT: NumField[] = [
  {
    key: 'max_sessions_per_ip',
    label: '每 IP 并发会话',
    hint: '每 IP 同时可看的频道数，超出时踢掉最久未用的那路；0 = 不限。仅在「IP 管控」开启时生效',
  },
  {
    key: 'session_ttl_secs',
    label: '会话超时',
    hint: '秒；该 IP 超过此时长没有播放请求即释放会话。仅在「IP 管控」开启时生效',
  },
  { key: 'l1_window_secs', label: '一级（持续）窗口', hint: '秒' },
  { key: 'l1_limit', label: '一级窗口次数', hint: '0 = 关闭该级' },
  { key: 'l2_window_secs', label: '二级（突发）窗口', hint: '秒' },
  { key: 'l2_limit', label: '二级窗口次数', hint: '0 = 关闭该级' },
  { key: 'cooldown_secs', label: '超限冷却', hint: '秒；期间返回 429 + Retry-After' },
  { key: 'violation_window_secs', label: '违规统计窗口', hint: '秒' },
  { key: 'violation_decay_secs', label: '违规衰减', hint: '秒；每满一个周期失效一条' },
  { key: 'ban_threshold', label: '升级封禁的违规次数' },
  { key: 'ban_duration_secs', label: '违规升级的封禁时长', hint: '秒，默认 30 天' },
];

const AUTH_LOCK: NumField[] = [
  { key: 'auth_fail_window_secs', label: '失败统计窗口', hint: '秒' },
  { key: 'auth_fail_free_per_window', label: '窗口内免费次数' },
  { key: 'auth_fail_ban_threshold', label: '免费后再失败几次锁定' },
  {
    key: 'auth_fail_ban_duration_secs',
    label: '第 2 次锁定时长',
    hint: '秒；第 1 次仍用「锁定秒数」，默认 300',
  },
  { key: 'auth_fail_ban_repeat_duration_secs', label: '第 3 次及以后锁定时长', hint: '秒' },
  {
    key: 'auth_fail_ban_escalate_window_secs',
    label: '锁定升级统计窗口',
    hint: '秒；超出窗口后重新按第 1 次计',
  },
];

function authNumber(auth: AuthCfg, key: string): string {
  const value = auth[key];
  return typeof value === 'number' ? String(value) : '';
}

function NumberField({
  field,
  value,
  onChange,
}: {
  field: NumField;
  value: string;
  onChange: (next: string) => void;
}) {
  return (
    <label className="field">
      <span>
        {field.label}
      </span>
      <input
        type="number"
        min={0}
        value={value}
        onChange={(event) => onChange(event.target.value)}
      />
      {field.hint && <div className="hint">{field.hint}</div>}
    </label>
  );
}

/** `[auth]` 四组编辑卡片：白名单、IP 管控与限流、会话与 Token、防爆破。 */
function AuthConfigCard({ onSaved }: { onSaved: () => void }) {
  const [config, setConfig] = useState<FullConfig | null>(null);
  const [saving, setSaving] = useState(false);
  const [note, setNote] = useState('');
  const [error, setError] = useState('');
  const [whitelist, setWhitelist] = useState('');

  const load = useCallback(async () => {
    try {
      const next = await api<FullConfig>('GET', '/config');
      setConfig(next);
      const list = Array.isArray(next.auth?.whitelist) ? next.auth.whitelist : [];
      setWhitelist(list.map((item) => String(item)).join('\n'));
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (error && !config) {
    return (
      <div className="card">
        <h2>访问控制</h2>
        <div className="msg err">{error}</div>
      </div>
    );
  }
  if (!config) {
    return <div className="card muted">读取配置…</div>;
  }

  const auth = config.auth ?? {};
  const setKey = (key: string, value: unknown) =>
    setConfig({ ...config, auth: { ...auth, [key]: value } });

  const save = async () => {
    setSaving(true);
    setNote('');
    setError('');
    try {
      const next: FullConfig = {
        ...config,
        auth: {
          ...auth,
          whitelist: whitelist
            .split('\n')
            .map((line) => line.trim())
            .filter(Boolean),
        },
      };
      await api('PUT', '/config', next);
      setNote('访问控制已保存并生效');
      await load();
      onSaved();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="card">
      <div className="row between">
        <div>
          <h2>访问控制</h2>
          <p className="muted">
            写 `[auth]` 段：改完点保存，整份配置原子替换（与设置页同一机制）。
          </p>
        </div>
        <button disabled={saving} onClick={() => void save()}>
          {saving ? '保存中…' : '保存访问控制'}
        </button>
      </div>

      <h3>白名单</h3>
      <p className="muted">
        每行一个地址，支持精确 IP、IPv4 CIDR 与 `*`。只在上方「IP 管控」开启时生效：
        名单内的地址豁免封禁与并发会话上限。目标键面里有它，但豁免语义在目标上未观测到，
        本实现的规则是自定的（见 `PROGRESS.md` §3.3）。
      </p>
      <textarea
        rows={4}
        placeholder="203.0.113.7&#10;10.0.0.0/8&#10;*"
        value={whitelist}
        onChange={(event) => setWhitelist(event.target.value)}
      />

      <h3>会话与 Token</h3>
      <div className="fields">
        <label className="field">
          <span>IP 管控</span>
          <select
            value={auth.ip_guard_enabled === true ? 'on' : 'off'}
            onChange={(event) => setKey('ip_guard_enabled', event.target.value === 'on')}
          >
            <option value="off">关闭</option>
            <option value="on">开启</option>
          </select>
        </label>
        <label className="field">
          <span>播放 Token</span>
          <select
            value={auth.token_enabled === true ? 'on' : 'off'}
            onChange={(event) => setKey('token_enabled', event.target.value === 'on')}
          >
            <option value="off">关闭</option>
            <option value="on">开启</option>
          </select>
        </label>
        <NumberField
          field={{ key: 'token_ttl_hours', label: 'Token 有效期', hint: '小时' }}
          value={authNumber(auth, 'token_ttl_hours')}
          onChange={(next) => setKey('token_ttl_hours', Number(next) || 0)}
        />
      </div>

      <h3>限流与会话</h3>
      <p className="muted">
        限流只计播放入口请求（换台 / 新建播放），不计切片。二级是突发上限，
        一级是持续上限，任一超限即进入冷却并记一次违规。全部只在「IP 管控」开启时生效。
        规则是自定的：目标 v0.8.2 的触发条件未复现（见 `PROGRESS.md` §3.1.5）。
      </p>
      <div className="fields">
        {AUTH_LIMIT.map((field) => (
          <NumberField
            key={field.key}
            field={field}
            value={authNumber(auth, field.key)}
            onChange={(next) => setKey(field.key, Number(next) || 0)}
          />
        ))}
      </div>
      <div className="fields">
        <label className="field">
          <span>限流跳转地址</span>
          <input
            value={typeof auth.limit_url === 'string' ? auth.limit_url : ''}
            onChange={(event) => setKey('limit_url', event.target.value)}
          />
        </label>
        <label className="field">
          <span>封禁跳转地址</span>
          <input
            value={typeof auth.ban_url === 'string' ? auth.ban_url : ''}
            onChange={(event) => setKey('ban_url', event.target.value)}
          />
        </label>
      </div>

      <h3>登录防爆破</h3>
      <p className="muted">
        窗口内累计失败达到「免费次数 + 阈值」后，该 IP 的 `/panel/api/login` 返回 `429`；
        锁不波及已有的 Bearer 会话。首次锁定时长固定 ≈300s（与目标观测一致）。
      </p>
      <div className="fields">
        {AUTH_LOCK.map((field) => (
          <NumberField
            key={field.key}
            field={field}
            value={authNumber(auth, field.key)}
            onChange={(next) => setKey(field.key, Number(next) || 0)}
          />
        ))}
      </div>

      {note && <div className="msg ok">{note}</div>}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

/** 安全管控：播放 Token 与 IP 封禁。 */
export function SecurityPage() {
  const [token, setToken] = useState<TokenInfo | null>(null);
  const [stats, setStats] = useState<IpStats | null>(null);
  const [ip, setIp] = useState('');
  const [seconds, setSeconds] = useState('3600');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const [nextToken, nextStats] = await Promise.all([
        api<TokenInfo>('GET', '/security/token'),
        api<IpStats>('GET', '/security/ip/stats'),
      ]);
      setToken(nextToken);
      setStats(nextStats);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const rotate = async () => {
    setMessage('');
    setError('');
    try {
      await api('POST', '/security/token/rotate');
      setMessage('Token 已轮换，旧值保留 1 小时宽限');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const ban = async () => {
    setMessage('');
    setError('');
    try {
      // 支持一次填多个地址（逗号、空格或换行分隔）。目标字段是 `ips` 数组。
      const ips = ip
        .split(/[\s,，;；]+/)
        .map((item) => item.trim())
        .filter((item) => item !== '');
      await api('POST', '/security/ip/ban', {
        ips,
        secs: Number(seconds) || 0,
      });
      setMessage(`已封禁 ${ips.length} 个地址`);
      setIp('');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const unban = async (target: string) => {
    setMessage('');
    setError('');
    try {
      await api('POST', '/security/ip/unban', { ip: target });
      setMessage(`已解封 ${target}`);
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  /** 批量解封：`POST /security/ip/unban-batch`，体为 `{ips:[…]}`。 */
  const unbanBatch = async () => {
    const ips = ip
      .split(/[\s,，;；]+/)
      .map((item) => item.trim())
      .filter((item) => item !== '');
    if (ips.length === 0) {
      setError('请先填写要解封的地址');
      return;
    }
    setMessage('');
    setError('');
    try {
      await api('POST', '/security/ip/unban-batch', { ips });
      setMessage(`已批量解封 ${ips.length} 个地址`);
      setIp('');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  return (
    <>
      <PageHead icon="security" title="安全管控" sub="播放 Token、访问控制与限流">
        <button className="ghost" onClick={() => void load()}>
          <Icon name="refresh" size={14} />
          刷新
        </button>
        <button onClick={() => void rotate()}>轮换 Token</button>
      </PageHead>
      <div className="card">
        <dl>
          <dt>校验</dt>
          <dd>{token ? (token.enabled ? '已开启' : '已关闭') : '—'}</dd>
          <dt>有效期</dt>
          <dd>{token ? `${token.ttl_hours} 小时` : '—'}</dd>
          <dt>当前值</dt>
          <dd className="mono ellipsis">{token?.current_value || token?.current || '—'}</dd>
          <dt>剩余</dt>
          <dd>
            {token?.status?.current
              ? `${Math.max(0, token.status.current.remaining_secs)} 秒`
              : '—'}
          </dd>
          <dt>旧值（宽限）</dt>
          <dd className="mono ellipsis">{token?.status?.old?.value ?? token?.old ?? '—'}</dd>
        </dl>
        {message && <div className="msg ok">{message}</div>}
        {error && <div className="msg err">{error}</div>}
      </div>

      <AuthConfigCard onSaved={() => void load()} />
      <LinkInfoCard />

      <div className="card">
        <h2>IP 统计与封禁</h2>
        <p className="muted">
          共 {stats?.count ?? 0} 个地址，其中封禁 {stats?.banned ?? 0} 个。封禁只作用于非面板路径。
        </p>
        <div className="row">
          <input
            placeholder="地址，可用逗号或换行分隔多个"
            value={ip}
            onChange={(event) => setIp(event.target.value)}
            style={{ flex: 2 }}
          />
          <input
            placeholder="秒"
            value={seconds}
            onChange={(event) => setSeconds(event.target.value)}
            style={{ flex: 1 }}
          />
          <button onClick={() => void ban()} disabled={ip.trim() === ''}>
            封禁
          </button>
          <button className="ghost" onClick={() => void unbanBatch()} disabled={ip.trim() === ''}>
            批量解封
          </button>
        </div>
        <table className="table" style={{ marginTop: 12 }}>
          <thead>
            <tr>
              <th>地址</th>
              <th className="right">请求</th>
              <th className="right">失败</th>
              <th>状态</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {(stats?.entries ?? []).map((entry) => (
              <tr key={entry.ip}>
                <td className="mono">{entry.ip}</td>
                <td className="right">{entry.requests}</td>
                <td className="right">{entry.failures}</td>
                <td>{entry.banned ? `封禁中（${entry.remaining_secs} 秒）` : '正常'}</td>
                <td className="right">
                  {entry.banned && (
                    <button className="ghost" onClick={() => void unban(entry.ip)}>
                      解封
                    </button>
                  )}
                </td>
              </tr>
            ))}
            {(stats?.entries ?? []).length === 0 && (
              <tr>
                <td colSpan={5} className="muted">
                  暂无记录。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </>
  );
}

/**
 * 日志级别：`tracing_subscriber::fmt` 的默认文本格式里级别是一个大写单词
 * （`2026-10-25T12:00:00Z  WARN tv_buddy::…: …`）。
 * 过滤在前端做：环形缓冲固定 500 行，按级别筛选不需要重新请求。
 */
const LEVELS = ['all', 'trace', 'debug', 'info', 'warn', 'error'] as const;
type Level = (typeof LEVELS)[number];

/** 环形缓冲里的行带 ANSI 颜色，直接塞进 `<pre>` 会显示成乱码控制符。 */
// eslint-disable-next-line no-control-regex
const ANSI = /\u001b\[[0-9;]*m/g;

function plain(line: string): string {
  return line.replace(ANSI, '');
}

function levelOf(line: string): string {
  const match = plain(line).match(/\s(TRACE|DEBUG|INFO|WARN|ERROR)\s/);
  return match ? match[1].toLowerCase() : '';
}

/** 运行日志：轮询 `/panel/api/logs`。 */
export function LogsPage() {
  const [lines, setLines] = useState<string[]>([]);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState('');
  const [limit, setLimit] = useState('200');
  const [level, setLevel] = useState<Level>('all');

  const load = useCallback(async () => {
    try {
      const data = await api<{ total: number; lines: string[] }>(
        'GET',
        `/logs?limit=${Number(limit) || 200}`,
      );
      setLines(data.lines);
      setTotal(data.total);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, [limit]);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 5000);
    return () => window.clearInterval(timer);
  }, [load]);

  const shown = level === 'all' ? lines : lines.filter((line) => levelOf(line) === level);

  return (
    <>
      <PageHead
        icon="logs"
        title="运行日志"
        sub={`最近 ${shown.length} 行${level === 'all' ? '' : `（${level.toUpperCase()}）`}（共 ${total} 行）`}
      >
        <div className="row">
          <select
            value={level}
            style={{ width: 120 }}
            onChange={(event) => setLevel(event.target.value as Level)}
          >
            {LEVELS.map((item) => (
              <option key={item} value={item}>
                {item === 'all' ? '全部级别' : item.toUpperCase()}
              </option>
            ))}
          </select>
          <input
            value={limit}
            onChange={(event) => setLimit(event.target.value)}
            style={{ width: 80 }}
          />
          <button className="ghost" onClick={() => void load()}>
            <Icon name="refresh" size={14} />
            刷新
          </button>
        </div>
      </PageHead>
      <div className="card">
        {error && <div className="msg err">{error}</div>}
        <pre className="log" style={{ marginTop: 0 }}>
          {shown.map(plain).join('\n')}
        </pre>
        {level !== 'all' && shown.length === 0 && (
          <p className="muted">当前缓冲里没有 {level.toUpperCase()} 级别的行。</p>
        )}
      </div>
    </>
  );
}

/** 备份与恢复。 */
export function BackupPage() {
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [scope, setScope] = useState('all');
  const file = useRef<HTMLInputElement>(null);

  const download = async () => {
    setMessage('');
    setError('');
    try {
      const response = await fetch(`/panel/api/backup?include=${scope}`, {
        headers: { Authorization: `Bearer ${getToken()}` },
      });
      if (!response.ok) {
        throw new Error(`HTTP ${response.status}`);
      }
      const blob = await response.blob();
      const disposition = response.headers.get('content-disposition') ?? '';
      const name = /filename="([^"]+)"/.exec(disposition)?.[1] ?? 'tv-buddy-backup.tar.gz';
      const url = URL.createObjectURL(blob);
      const anchor = document.createElement('a');
      anchor.href = url;
      anchor.download = name;
      anchor.click();
      URL.revokeObjectURL(url);
      setMessage(`已导出 ${name}（${blob.size} 字节）`);
    } catch (err) {
      setError(errorText(err));
    }
  };

  const restore = async (input: HTMLInputElement) => {
    const selected = input.files?.[0];
    if (!selected) {
      return;
    }
    setMessage('');
    setError('');
    try {
      const response = await fetch('/panel/api/restore', {
        method: 'POST',
        headers: {
          Authorization: `Bearer ${getToken()}`,
          'Content-Type': 'application/gzip',
        },
        body: await selected.arrayBuffer(),
      });
      const data = (await response.json()) as { error?: string; report?: { files: string[] } };
      if (!response.ok) {
        throw new Error(data.error ?? `HTTP ${response.status}`);
      }
      setMessage(`已恢复 ${data.report?.files.length ?? 0} 个文件，配置已重载`);
    } catch (err) {
      setError(errorText(err));
    } finally {
      input.value = '';
    }
  };

  return (
    <>
      <PageHead icon="download" title="备份与恢复" sub="导出与回滚配置、脚本与资源" />
      <div className="card">
        <div className="section-title">导出</div>
        <div className="row">
        <select value={scope} onChange={(event) => setScope(event.target.value)}>
          <option value="all">全部</option>
          <option value="epg-scripts">仅 EPG 脚本</option>
          <option value="resolver">仅解析脚本</option>
        </select>
        <button onClick={() => void download()}>下载备份</button>
      </div>
        <div className="section-title" style={{ marginTop: 20 }}>
          恢复
        </div>
        <p className="muted">
          恢复会覆盖 `config.toml`，旧文件另存为 `config.toml.bak`。非法内容不会落盘。
        </p>
        <input
          ref={file}
          type="file"
          accept=".gz,application/gzip"
          onChange={(event) => void restore(event.target)}
        />
        {message && <div className="msg ok">{message}</div>}
        {error && <div className="msg err">{error}</div>}
      </div>
    </>
  );
}

type EpgStatus = {
  enabled: boolean;
  running: boolean;
  sources: number;
  channels: number;
  programmes: number;
  kept: number;
  keep_days: number;
  future_days: number;
  last_success: number | null;
  last_error: string | null;
};

type EpgMapping = {
  source_id: string;
  source_channel_id: string;
  source_name: string;
  source_display_name: string;
};

type EpgExtra = {
  source_id: string;
  source_channel_id: string;
  out_id: string;
  out_name: string;
};

type EpgMapData = {
  mappings: Record<string, EpgMapping>;
  extra: EpgExtra[];
};

/** `[epg.translate]` 的运行时视图（`GET /panel/api/epg/translate/status`）。 */
type TranslateStatus = {
  enabled: boolean;
  engine: string;
  url: string;
  source: string;
  targets: string[];
  translation_first: boolean;
  cache: number;
};

/** 翻译卡片：`[epg.translate]` 的关键键 + 立即翻译 + 清缓存。 */
function EpgTranslateCard({ onChanged }: { onChanged: () => void }) {
  const [config, setConfig] = useState<Record<string, unknown> | null>(null);
  const [status, setStatus] = useState<TranslateStatus | null>(null);
  const [note, setNote] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);
  const [targets, setTargets] = useState('');

  const load = useCallback(async () => {
    try {
      const [nextConfig, nextStatus] = await Promise.all([
        api<Record<string, unknown>>('GET', '/config'),
        api<TranslateStatus>('GET', '/epg/translate/status'),
      ]);
      setConfig(nextConfig);
      setStatus(nextStatus);
      const epg = (nextConfig.epg ?? {}) as Record<string, unknown>;
      const translate = (epg.translate ?? {}) as Record<string, unknown>;
      const list = Array.isArray(translate.targets)
        ? translate.targets
        : typeof translate.target === 'string'
          ? [translate.target]
          : [];
      setTargets(list.map((item) => String(item)).join('\n'));
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  if (!config) {
    return (
      <div className="card">
        <h2>EPG 翻译</h2>
        {error ? <div className="msg err">{error}</div> : <p className="muted">读取中…</p>}
      </div>
    );
  }

  const epg = (config.epg ?? {}) as Record<string, unknown>;
  const translate = (epg.translate ?? {}) as Record<string, unknown>;

  /** 改 `[epg.translate]` 的一个键（整份回写，同设置页机制）。 */
  const setKey = (key: string, value: unknown) =>
    setConfig({ ...config, epg: { ...epg, translate: { ...translate, [key]: value } } });

  const num = (key: string) => {
    const value = translate[key];
    return typeof value === 'number' ? String(value) : '';
  };
  const text = (key: string) => (typeof translate[key] === 'string' ? String(translate[key]) : '');

  const save = async () => {
    setBusy(true);
    setNote('');
    setError('');
    try {
      const list = targets
        .split('\n')
        .map((line) => line.trim())
        .filter(Boolean);
      const next = {
        ...config,
        epg: {
          ...epg,
          translate: {
            ...translate,
            targets: list,
            // `target` 是单目标别名；多目标时指向第一个，保持键面语义一致。
            target: list[0] ?? '',
          },
        },
      };
      await api('PUT', '/config', next);
      setNote('EPG 翻译配置已保存');
      await load();
      onChanged();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const runNow = async () => {
    setBusy(true);
    setNote('');
    setError('');
    try {
      const result = await api<{ channels: number; programmes: number; translated: number }>(
        'POST',
        '/epg/translate/run',
      );
      setNote(
        `已翻译：${result.channels} 个频道、${result.programmes} 条节目、${result.translated} 条译文`,
      );
      await load();
      onChanged();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const clearCache = async () => {
    setBusy(true);
    setNote('');
    setError('');
    try {
      const result = await api<{ cleared: number }>('POST', '/epg/translate/clear-engine');
      setNote(`已清空翻译缓存（${result.cleared} 个文件）`);
      await load();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const testMtran = async () => {
    setBusy(true);
    setNote('');
    setError('');
    try {
      const result = await api<{ ok: boolean; result?: string; error?: string }>(
        'POST',
        '/epg/translate/mtran/test',
        { text: 'hello' },
      );
      if (result.ok) setNote(`MTranServer 可用：「hello」→「${result.result ?? ''}」`);
      else setError(result.error || 'MTranServer 不可用');
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card">
      <div className="row between">
        <div>
          <h2>EPG 翻译</h2>
          <p className="muted">
            引擎负责把节目名/简介翻成目标语言，译文按 `lang` 写进 XMLTV。
            本实现只实现 MTranServer 的 HTTP 批量接口（`/translate/batch`，失败回退单条）。
          </p>
        </div>
        <div className="row">
          <button className="ghost" disabled={busy} onClick={() => void testMtran()}>
            测试连接
          </button>
          <button className="ghost" disabled={busy} onClick={() => void clearCache()}>
            清空译文缓存
          </button>
          <button disabled={busy} onClick={() => void save()}>
            {busy ? '处理中…' : '保存翻译设置'}
          </button>
        </div>
      </div>

      <div className="fields">
        <label className="field">
          <span>启用</span>
          <select
            value={translate.enabled === true ? 'on' : 'off'}
            onChange={(event) => setKey('enabled', event.target.value === 'on')}
          >
            <option value="off">关闭</option>
            <option value="on">开启</option>
          </select>
        </label>
        <label className="field">
          <span>引擎</span>
          <select
            value={text('engine') || 'mtran'}
            onChange={(event) => setKey('engine', event.target.value)}
          >
            <option value="mtran">MTranServer</option>
            <option value="google">google（目标键面；本实现仅走 URL 的 HTTP 接口）</option>
          </select>
        </label>
        <label className="field">
          <span>MTranServer 地址</span>
          <input
            placeholder="http://127.0.0.1:8989"
            value={text('mtran_url')}
            onChange={(event) => setKey('mtran_url', event.target.value)}
          />
        </label>
        <label className="field">
          <span>Token（可选）</span>
          <input
            value={text('mtran_token')}
            onChange={(event) => setKey('mtran_token', event.target.value)}
          />
        </label>
        <label className="field">
          <span>批量大小</span>
          <input
            type="number"
            min={1}
            max={500}
            value={num('mtran_batch')}
            onChange={(event) => setKey('mtran_batch', Number(event.target.value) || 0)}
          />
        </label>
        <label className="field">
          <span>每轮上限</span>
          <input
            type="number"
            min={0}
            value={num('mtran_max_per_round')}
            onChange={(event) => setKey('mtran_max_per_round', Number(event.target.value) || 0)}
          />
        </label>
      </div>
      <div className="fields">
        <label className="field">
          <span>源语言</span>
          <input
            value={text('mtran_source') || 'auto'}
            onChange={(event) => setKey('mtran_source', event.target.value)}
          />
        </label>
        <label className="field">
          <span>目标语言（每行一个）</span>
          <textarea
            rows={3}
            placeholder="zh-cn"
            value={targets}
            onChange={(event) => setTargets(event.target.value)}
          />
        </label>
        <label className="field">
          <span>译文位置</span>
          <select
            value={translate.translation_first === true ? 'first' : 'second'}
            onChange={(event) => setKey('translation_first', event.target.value === 'first')}
          >
            <option value="first">译文在上（原文在下）</option>
            <option value="second">原文在上</option>
          </select>
        </label>
      </div>

      <div className="row">
        <button className="ghost" disabled={busy} onClick={() => void runNow()}>
          立即翻译
        </button>
        <span className="muted">
          状态：{status?.enabled ? '已启用' : '未启用'}；引擎 {status?.engine ?? '—'}；
          目标 {status?.targets?.join('/') || '—'}；缓存 {status?.cache ?? 0} 条
        </span>
      </div>

      {note && <div className="msg ok">{note}</div>}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/** 采集节奏：`[epg].cron` + `cron-preview` 校验（A5 的下半部分）。 */
function EpgCronCard({ onChanged }: { onChanged: () => void }) {
  const [expr, setExpr] = useState('');
  const [saved, setSaved] = useState('');
  const [next, setNext] = useState<string[]>([]);
  const [note, setNote] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const config = await api<{ epg?: { cron?: string } }>('GET', '/config');
      const cron = config.epg?.cron ?? '';
      setExpr(cron);
      setSaved(cron);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const preview = async () => {
    setNote('');
    setError('');
    try {
      const result = await api<{ ok: boolean; next?: string[]; error?: string }>(
        'POST',
        '/epg/cron-preview',
        { expr },
      );
      if (result.ok) {
        setNext(result.next ?? []);
        setNote(`表达式合法，接下来 ${result.next?.length ?? 0} 次触发如下`);
      } else {
        setNext([]);
        setError(result.error ?? '表达式不合法');
      }
    } catch (err) {
      setError(errorText(err));
    }
  };

  const save = async () => {
    setBusy(true);
    setNote('');
    setError('');
    try {
      const config = await api<Record<string, unknown>>('GET', '/config');
      const epg = (config.epg ?? {}) as Record<string, unknown>;
      // `[epg]` 段（含源列表）在启动时读取，改 cron 也不会立刻换调度；界面上写明。
      await api('PUT', '/config', { ...config, epg: { ...epg, cron: expr.trim() } });
      setSaved(expr.trim());
      setNote('采集节奏已保存；`[epg]` 段需重启后重新排程');
      onChanged();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card">
      <h2>采集节奏</h2>
      <p className="muted">
        cron 表达式（5 段：分 时 日 月 周），按 `[server].timezone_offset_hours` 解释。
        默认 `0 */4 * * *`。`[epg]` 段在启动时读取，改动需重启才生效。
      </p>
      <div className="row">
        <input
          className="mono"
          style={{ flex: 2 }}
          value={expr}
          onChange={(event) => setExpr(event.target.value)}
        />
        <button className="ghost" onClick={() => void preview()}>
          预览
        </button>
        <button disabled={busy || expr.trim() === saved} onClick={() => void save()}>
          {busy ? '保存中…' : '保存'}
        </button>
      </div>
      {next.length > 0 && (
        <ul className="muted">
          {next.map((item) => (
            <li key={item} className="mono">
              {item}
            </li>
          ))}
        </ul>
      )}
      {note && <div className="msg ok">{note}</div>}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/** 节目指南：采集状态、采集节奏、翻译设置、手工映射与额外频道。 */
export function EpgPanelPage() {
  const [status, setStatus] = useState<EpgStatus | null>(null);
  const [data, setData] = useState<EpgMapData>({ mappings: {}, extra: [] });
  const [mapping, setMapping] = useState({
    gid: '',
    cid: '',
    source_id: '',
    source_channel_id: '',
  });
  const [extra, setExtra] = useState({
    source_id: '',
    source_channel_id: '',
    out_id: '',
    out_name: '',
  });
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const [nextStatus, nextData] = await Promise.all([
        api<EpgStatus>('GET', '/epg/status'),
        api<EpgMapData>('GET', '/epg/mappings'),
      ]);
      setStatus(nextStatus);
      setData(nextData);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const refresh = async () => {
    setBusy(true);
    setMessage('');
    setError('');
    try {
      const result = await api<{ channels: number; programmes: number }>(
        'POST',
        '/epg/refresh',
      );
      setMessage(`采集完成：${result.channels} 个频道，${result.programmes} 条节目`);
      await load();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const saveMapping = async () => {
    setMessage('');
    setError('');
    try {
      await api('POST', '/epg/mapping', mapping);
      setMapping({ gid: '', cid: '', source_id: '', source_channel_id: '' });
      setMessage('映射已保存');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const removeMapping = async (key: string) => {
    const [gid, cid] = key.split('/');
    try {
      await api('POST', '/epg/mapping', { gid, cid, source_id: '', source_channel_id: '' });
      setMessage('映射已清除');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const addExtra = async () => {
    setMessage('');
    setError('');
    try {
      await api('POST', '/epg/extra', { action: 'add', ...extra });
      setExtra({ source_id: '', source_channel_id: '', out_id: '', out_name: '' });
      setMessage('额外频道已保存，正在重新生成');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const removeExtra = async (item: EpgExtra) => {
    try {
      await api('POST', '/epg/extra', {
        action: 'remove',
        source_id: item.source_id,
        source_channel_id: item.source_channel_id,
      });
      setMessage('额外频道已删除');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const regenerate = async () => {
    setBusy(true);
    setMessage('');
    setError('');
    try {
      const result = await api<{ channels: number; programmes: number }>(
        'POST',
        '/epg/regenerate',
      );
      setMessage(`重新生成：${result.channels} 个频道，${result.programmes} 条节目`);
      await load();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  const last = status?.last_success
    ? new Date(status.last_success * 1000).toLocaleString()
    : '—';

  return (
    <>
      <PageHead
        icon="epg"
        title="节目指南"
        sub={status?.enabled ? '已开启' : '已关闭'}
      >
        <a className="ghost" href="/epg" target="_blank" rel="noreferrer">
          浏览页面
        </a>
        <button className="ghost" onClick={() => void load()}>
          <Icon name="refresh" size={14} />
          刷新状态
        </button>
        <button className="ghost" onClick={() => void regenerate()} disabled={busy}>
          重新生成
        </button>
        <button onClick={() => void refresh()} disabled={busy || !status?.enabled}>
          {busy ? '采集中…' : '立即采集'}
        </button>
      </PageHead>
      <div className="card">
        <dl>
          <dt>源</dt>
          <dd>{status?.sources ?? 0} 个</dd>
          <dt>频道 / 节目</dt>
          <dd>
            {status?.channels ?? 0} / {status?.programmes ?? 0}
          </dd>
          <dt>保留</dt>
          <dd>
            已过 {status?.keep_days ?? 0} 天、未来 {status?.future_days ?? 0} 天
          </dd>
          <dt>最近成功</dt>
          <dd>{last}</dd>
          <dt>最近错误</dt>
          <dd className="mono ellipsis">{status?.last_error ?? '—'}</dd>
        </dl>
        {message && <div className="msg ok">{message}</div>}
        {error && <div className="msg err">{error}</div>}
      </div>
      <EpgCronCard onChanged={() => void load()} />
      <EpgTranslateCard onChanged={() => void load()} />
      <EpgModelsCard />
      <EpgScriptsCard />
      <EpgMatchReportCard />
      <EpgViewerCard />
      <div className="card">
        <h2>手工映射</h2>
        <div className="row">
          <input
            placeholder="gid"
            value={mapping.gid}
            onChange={(event) => setMapping({ ...mapping, gid: event.target.value })}
          />
          <input
            placeholder="cid"
            value={mapping.cid}
            onChange={(event) => setMapping({ ...mapping, cid: event.target.value })}
          />
          <input
            placeholder="source_id"
            value={mapping.source_id}
            onChange={(event) => setMapping({ ...mapping, source_id: event.target.value })}
          />
          <input
            placeholder="source_channel_id"
            value={mapping.source_channel_id}
            onChange={(event) =>
              setMapping({ ...mapping, source_channel_id: event.target.value })
            }
          />
          <button onClick={() => void saveMapping()}>保存映射</button>
        </div>
        <table className="table">
          <thead>
            <tr>
              <th>频道</th>
              <th>源</th>
              <th>源频道</th>
              <th>源名称</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {Object.entries(data.mappings).map(([key, item]) => (
              <tr key={key}>
                <td className="mono">{key}</td>
                <td className="mono">{item.source_id}</td>
                <td className="mono">{item.source_channel_id}</td>
                <td>{item.source_display_name || item.source_name || '—'}</td>
                <td className="right">
                  <button className="icon-btn" onClick={() => void removeMapping(key)}>
                    清除
                  </button>
                </td>
              </tr>
            ))}
            {Object.keys(data.mappings).length === 0 && (
              <tr>
                <td colSpan={5} className="muted">
                  暂无手工映射。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>

      <EpgMatchToolsCard onPick={(pick) => setMapping(pick)} />

      <div className="card">
        <h2>额外频道</h2>
        <div className="row">
          <input
            placeholder="source_id"
            value={extra.source_id}
            onChange={(event) => setExtra({ ...extra, source_id: event.target.value })}
          />
          <input
            placeholder="source_channel_id"
            value={extra.source_channel_id}
            onChange={(event) => setExtra({ ...extra, source_channel_id: event.target.value })}
          />
          <input
            placeholder="out_id（可空）"
            value={extra.out_id}
            onChange={(event) => setExtra({ ...extra, out_id: event.target.value })}
          />
          <input
            placeholder="out_name（可空）"
            value={extra.out_name}
            onChange={(event) => setExtra({ ...extra, out_name: event.target.value })}
          />
          <button onClick={() => void addExtra()}>添加额外频道</button>
        </div>
        <table className="table">
          <thead>
            <tr>
              <th>源</th>
              <th>源频道</th>
              <th>输出 ID</th>
              <th>输出名称</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {data.extra.map((item) => (
              <tr key={`${item.source_id}/${item.source_channel_id}`}>
                <td className="mono">{item.source_id}</td>
                <td className="mono">{item.source_channel_id}</td>
                <td className="mono">{item.out_id || '—'}</td>
                <td>{item.out_name || '—'}</td>
                <td className="right">
                  <button className="icon-btn" onClick={() => void removeExtra(item)}>
                    删除
                  </button>
                </td>
              </tr>
            ))}
            {data.extra.length === 0 && (
              <tr>
                <td colSpan={5} className="muted">
                  暂无额外频道。
                </td>
              </tr>
            )}
          </tbody>
        </table>
      </div>
    </>
  );
}

/** EPG 采集脚本管理：`GET /epg/scripts`、`POST /epg/script`、`POST /epg/script-delete`。 */
function EpgScriptsCard() {
  const [python, setPython] = useState('');
  const [scripts, setScripts] = useState<{ name: string; size: number }[]>([]);
  const [name, setName] = useState('');
  const [content, setContent] = useState('');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const data = await api<{ python: string; scripts: { name: string; size: number }[] }>(
        'GET',
        '/epg/scripts',
      );
      setPython(data.python ?? '');
      setScripts(data.scripts ?? []);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const save = async () => {
    setMessage('');
    setError('');
    try {
      await api('POST', '/epg/script', { name: name.trim(), content });
      setMessage(`脚本 ${name.trim()} 已保存`);
      setName('');
      setContent('');
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  const remove = async (target: string) => {
    if (!window.confirm(`确认删除脚本「${target}」？`)) return;
    setMessage('');
    setError('');
    try {
      await api('POST', '/epg/script-delete', { name: target });
      setMessage(`脚本 ${target} 已删除`);
      await load();
    } catch (err) {
      setError(errorText(err));
    }
  };

  return (
    <div className="card">
      <div className="row between">
        <h2>采集脚本</h2>
        <span className="muted">{python ? `Python ${python}` : 'Python —'}</span>
      </div>
      <table className="table">
        <thead>
          <tr>
            <th>文件</th>
            <th className="right">大小</th>
            <th className="right">操作</th>
          </tr>
        </thead>
        <tbody>
          {scripts.map((script) => (
            <tr key={script.name}>
              <td className="mono">{script.name}</td>
              <td className="right">{Math.max(1, Math.round(script.size / 1024))} KB</td>
              <td className="right">
                <button
                  className="icon-btn danger-text"
                  onClick={() => void remove(script.name)}
                >
                  删除
                </button>
              </td>
            </tr>
          ))}
          {scripts.length === 0 && (
            <tr>
              <td colSpan={3} className="muted">
                暂无脚本。
              </td>
            </tr>
          )}
        </tbody>
      </table>
      <div className="row" style={{ marginTop: 8 }}>
        <input
          placeholder="脚本名，例如 mysource.py"
          value={name}
          onChange={(event) => setName(event.target.value)}
          style={{ flex: 1 }}
        />
        <button onClick={() => void save()} disabled={name.trim() === ''}>
          上传/覆盖
        </button>
      </div>
      <textarea
        placeholder="脚本正文"
        value={content}
        onChange={(event) => setContent(event.target.value)}
        rows={8}
        style={{ width: '100%', marginTop: 8 }}
      />
      {message && <div className="msg ok">{message}</div>}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/** 匹配报告：目标是 `text/plain` 表格，这里原样显示。 */
function EpgMatchReportCard() {
  const [text, setText] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      setText(await apiText('GET', '/epg/match-report'));
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="card">
      <div className="row between">
        <h2>匹配报告</h2>
        <button className="ghost" onClick={() => void load()}>
          刷新
        </button>
      </div>
      <pre
        className="mono"
        style={{ whiteSpace: 'pre-wrap', maxHeight: 360, overflow: 'auto' }}
      >
        {text || '—'}
      </pre>
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/** 翻译模型与探测：`GET /epg/translate/models`、`POST /epg/translate/probe`。 */
function EpgModelsCard() {
  const [models, setModels] = useState<{ enabled: boolean; engine: string; models: string[] }>({
    enabled: false,
    engine: 'none',
    models: [],
  });
  const [probe, setProbe] = useState('');
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      setModels(
        await api<{ enabled: boolean; engine: string; models: string[] }>(
          'GET',
          '/epg/translate/models',
        ),
      );
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const run = async () => {
    setProbe('');
    setError('');
    try {
      const result = await api<unknown>('POST', '/epg/translate/probe');
      setProbe(JSON.stringify(result, null, 2));
    } catch (err) {
      setError(errorText(err));
    }
  };

  return (
    <div className="card">
      <div className="row between">
        <h2>翻译模型</h2>
        <button className="ghost" onClick={() => void run()}>
          探测引擎
        </button>
      </div>
      <dl>
        <dt>启用</dt>
        <dd>{models.enabled ? '是' : '否'}</dd>
        <dt>引擎</dt>
        <dd className="mono">{models.engine}</dd>
        <dt>目标语言</dt>
        <dd className="mono">{models.models.join(', ') || '—'}</dd>
      </dl>
      {probe && (
        <pre className="mono" style={{ whiteSpace: 'pre-wrap' }}>
          {probe}
        </pre>
      )}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/**
 * 匹配工具：建议、源频道浏览、时区预览。
 *
 * 接口：`GET /epg/suggest/{gid}/{cid}`、`GET /epg/source-channels/{id}`、
 * `GET /epg/tz-preview/{id}?shift=`。
 */
function EpgMatchToolsCard({
  onPick,
}: {
  onPick: (pick: { gid: string; cid: string; source_id: string; source_channel_id: string }) => void;
}) {
  const [gid, setGid] = useState('');
  const [cid, setCid] = useState('');
  const [sourceId, setSourceId] = useState('__all__');
  const [query, setQuery] = useState('');
  const [shift, setShift] = useState('0');
  const [suggestions, setSuggestions] = useState<
    {
      source_id: string;
      source_channel_id: string;
      source_name: string;
      display_name: string;
      score: number;
    }[]
  >([]);
  const [items, setItems] = useState<
    { id: string; display_names: string[]; source_id: string; source_name: string }[]
  >([]);
  const [total, setTotal] = useState(0);
  const [preview, setPreview] = useState<{ raw: string; current: string; shifted: string } | null>(
    null,
  );
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');

  const runSuggest = async () => {
    setMessage('');
    setError('');
    try {
      const data = await api<{
        suggestions: {
          source_id: string;
          source_channel_id: string;
          source_name: string;
          display_name: string;
          score: number;
        }[];
      }>('GET', `/epg/suggest/${encodeURIComponent(gid)}/${encodeURIComponent(cid)}`);
      setSuggestions(data.suggestions ?? []);
      setMessage(`找到 ${(data.suggestions ?? []).length} 条建议`);
    } catch (err) {
      setError(errorText(err));
    }
  };

  const runSource = async () => {
    setMessage('');
    setError('');
    try {
      const params = new URLSearchParams({ q: query, limit: '60' });
      const data = await api<{
        items: { id: string; display_names: string[]; source_id: string; source_name: string }[];
        total: number;
      }>('GET', `/epg/source-channels/${encodeURIComponent(sourceId)}?${params.toString()}`);
      setItems(data.items ?? []);
      setTotal(data.total ?? 0);
      setMessage(`共 ${data.total ?? 0} 个源频道`);
    } catch (err) {
      setError(errorText(err));
    }
  };

  const runTz = async () => {
    setMessage('');
    setError('');
    try {
      const data = await api<{
        ok: boolean;
        items?: { raw: string; current: string; shifted: string }[];
        error?: string;
      }>('GET', `/epg/tz-preview/${encodeURIComponent(sourceId)}?shift=${Number(shift) || 0}`);
      if (!data.ok) {
        setError(data.error ?? '预览失败');
        return;
      }
      setPreview((data.items ?? [])[0] ?? null);
    } catch (err) {
      setError(errorText(err));
    }
  };

  return (
    <div className="card">
      <h2>匹配工具</h2>
      <div className="row">
        <input
          placeholder="gid"
          value={gid}
          onChange={(event) => setGid(event.target.value)}
          style={{ flex: 1 }}
        />
        <input
          placeholder="cid"
          value={cid}
          onChange={(event) => setCid(event.target.value)}
          style={{ flex: 1 }}
        />
        <button
          className="ghost"
          onClick={() => void runSuggest()}
          disabled={gid.trim() === '' || cid.trim() === ''}
        >
          匹配建议
        </button>
      </div>
      {suggestions.length > 0 && (
        <table className="table" style={{ marginTop: 8 }}>
          <thead>
            <tr>
              <th>源</th>
              <th>源频道</th>
              <th>名称</th>
              <th className="right">相似度</th>
              <th className="right">操作</th>
            </tr>
          </thead>
          <tbody>
            {suggestions.map((item) => (
              <tr key={`${item.source_id}/${item.source_channel_id}/${item.display_name}`}>
                <td className="mono">{item.source_name || item.source_id}</td>
                <td className="mono">{item.source_channel_id}</td>
                <td>{item.display_name}</td>
                <td className="right">{item.score}</td>
                <td className="right">
                  <button
                    className="icon-btn"
                    onClick={() =>
                      onPick({
                        gid: gid.trim(),
                        cid: cid.trim(),
                        source_id: item.source_id,
                        source_channel_id: item.source_channel_id,
                      })
                    }
                  >
                    填入映射
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      )}

      <div className="row" style={{ marginTop: 12 }}>
        <input
          placeholder="source_id（__all__ = 全部）"
          value={sourceId}
          onChange={(event) => setSourceId(event.target.value)}
          style={{ flex: 1 }}
        />
        <input
          placeholder="搜索词"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          style={{ flex: 1 }}
        />
        <button className="ghost" onClick={() => void runSource()}>
          浏览源频道
        </button>
      </div>
      {items.length > 0 && (
        <>
          <p className="muted">共 {total} 个，显示前 {items.length} 个。</p>
          <ul>
            {items.map((item) => (
              <li key={`${item.source_id}/${item.id}`} className="mono">
                {item.source_name}/{item.id} — {item.display_names.join(' / ') || '—'}
              </li>
            ))}
          </ul>
        </>
      )}

      <div className="row" style={{ marginTop: 12 }}>
        <input
          placeholder="偏移分钟"
          value={shift}
          onChange={(event) => setShift(event.target.value)}
          style={{ flex: 1 }}
        />
        <button className="ghost" onClick={() => void runTz()}>
          时区预览
        </button>
      </div>
      {preview && (
        <dl>
          <dt>原文</dt>
          <dd className="mono">{preview.raw}</dd>
          <dt>本机</dt>
          <dd className="mono">{preview.current}</dd>
          <dt>偏移后</dt>
          <dd className="mono">{preview.shifted}</dd>
        </dl>
      )}
      {message && <div className="msg ok">{message}</div>}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/** 公开查看器频道集合：`GET /epg/viewer-channels`。 */
function EpgViewerCard() {
  const [channels, setChannels] = useState<
    { id: string; name: string; source_name: string; programme_count: number }[]
  >([]);
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      const data = await api<{
        channels: { id: string; name: string; source_name: string; programme_count: number }[];
      }>('GET', '/epg/viewer-channels');
      setChannels(data.channels ?? []);
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <div className="card">
      <div className="row between">
        <h2>公开查看器频道</h2>
        <button className="ghost" onClick={() => void load()}>
          刷新
        </button>
      </div>
      <p className="muted">
        共 {channels.length} 个频道。公开页面见 <code>/epg</code>。
      </p>
      <ul>
        {channels.slice(0, 50).map((channel) => (
          <li key={channel.id}>
            <span className="mono">{channel.id}</span> {channel.name}
            <span className="muted">
              {' '}
              · {channel.source_name || '—'} · {channel.programme_count} 条节目
            </span>
          </li>
        ))}
      </ul>
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}

/** 播放链接审计：`GET /panel/api/link-info`。 */function LinkInfoCard() {
  const [info, setInfo] = useState<{
    aliases: Record<string, unknown>;
    random_id_enabled: boolean;
    token: string;
    token_enabled: boolean;
    use_alias: boolean;
  } | null>(null);
  const [error, setError] = useState('');

  useEffect(() => {
    void (async () => {
      try {
        setInfo(
          await api<{
            aliases: Record<string, unknown>;
            random_id_enabled: boolean;
            token: string;
            token_enabled: boolean;
            use_alias: boolean;
          }>('GET', '/link-info'),
        );
      } catch (err) {
        setError(errorText(err));
      }
    })();
  }, []);

  const aliases = Object.keys(info?.aliases ?? {});

  return (
    <div className="card">
      <h2>播放链接审计</h2>
      <dl>
        <dt>Token 校验</dt>
        <dd>{info?.token_enabled ? '已开启' : '已关闭'}</dd>
        <dt>当前 Token</dt>
        <dd className="mono ellipsis">{info?.token || '—'}</dd>
        <dt>随机 ID</dt>
        <dd>{info?.random_id_enabled ? '已开启' : '已关闭'}</dd>
        <dt>别名</dt>
        <dd className="mono">{aliases.length > 0 ? aliases.join(', ') : '—'}</dd>
      </dl>
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}
