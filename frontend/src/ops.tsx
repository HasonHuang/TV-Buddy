/**
 * 运维分区：安全管控、运行日志、备份与恢复。
 *
 * 接口：`/panel/api/security/*`、`/panel/api/logs`、`/panel/api/{backup,restore}`。
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import { api, apiText, getToken, putConfig } from './api';
import { EpgBindingsTab, EpgRulesTab, EpgSourcesTab } from './epg';
import { Icon } from './icons';
import { Badge, Confirm, Field, NumberBox, PageHead, Select, Tip } from './ui';

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

/** 数字字段：路径 + 标签 + 单位 + 说明。 */
type NumField = { key: string; label: string; hint?: string; unit?: string; tip?: string };

const AUTH_LIMIT: NumField[] = [
  {
    key: 'max_sessions_per_ip',
    label: '每 IP 并发会话',
    unit: '路',
    hint: '每 IP 同时可看的频道数。超出时踢掉最久未用的那路；0 = 不限',
  },
  {
    key: 'session_ttl_secs',
    label: '会话超时',
    unit: '秒',
    hint: '该 IP 超过此时长没有播放请求，即释放会话',
  },
  { key: 'l1_window_secs', label: '一级窗口', unit: '秒', hint: '持续上限的统计窗口' },
  { key: 'l1_limit', label: '一级次数', unit: '次', hint: '0 = 关闭该级' },
  { key: 'l2_window_secs', label: '二级窗口', unit: '秒', hint: '突发上限的统计窗口' },
  { key: 'l2_limit', label: '二级次数', unit: '次', hint: '0 = 关闭该级' },
  {
    key: 'cooldown_secs',
    label: '超限冷却',
    unit: '秒',
    hint: '冷却期间返回 429 + Retry-After',
  },
  { key: 'violation_window_secs', label: '违规统计窗口', unit: '秒', hint: '只统计窗口内的违规' },
  {
    key: 'violation_decay_secs',
    label: '违规衰减',
    unit: '秒',
    hint: '每满一个周期失效一条违规',
  },
  {
    key: 'ban_threshold',
    label: '违规封禁阈值',
    unit: '次',
    hint: '有效违规达到该次数即本机封禁',
  },
  { key: 'ban_duration_secs', label: '封禁时长', unit: '秒', hint: '默认 30 天（2592000 秒）' },
];

const AUTH_LOCK: NumField[] = [
  { key: 'auth_fail_window_secs', label: '失败统计窗口', unit: '秒', hint: '只统计窗口内的失败' },
  {
    key: 'auth_fail_free_per_window',
    label: '失败容忍次数',
    unit: '次',
    hint: '窗口内前几次失败不触发锁定；0 = 不容忍',
  },
  {
    key: 'auth_fail_ban_threshold',
    label: '追加失败次数',
    unit: '次',
    hint: '超过容忍次数后，再失败几次即锁定',
  },
  {
    key: 'auth_fail_lock_secs',
    label: '首次锁定时长',
    unit: '秒',
    hint: '默认 5 分钟（300 秒）',
  },
  {
    key: 'auth_fail_ban_duration_secs',
    label: '第 2 次锁定时长',
    unit: '秒',
    hint: '仅在「IP 管控」开启时按阶梯升级',
  },
  {
    key: 'auth_fail_ban_repeat_duration_secs',
    label: '第 3 次及以后锁定',
    unit: '秒',
    hint: '默认 30 天（2592000 秒）',
  },
  {
    key: 'auth_fail_ban_escalate_window_secs',
    label: '锁定升级统计窗口',
    unit: '秒',
    hint: '超过窗口未再锁定，按第 1 次计',
  },
];

/** 32 字节随机密钥，十六进制小写。 */
function randomSecret(): string {
  const bytes = new Uint8Array(32);
  crypto.getRandomValues(bytes);
  return Array.from(bytes, (byte) => byte.toString(16).padStart(2, '0')).join('');
}

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
    <Field label={field.label} hint={field.hint} tip={field.tip}>
      <div className="field-control">
        <NumberBox
          min={0}
          ariaLabel={field.label}
          value={value}
          onChange={onChange}
        />
        {field.unit ? <span className="field-unit">{field.unit}</span> : null}
      </div>
    </Field>
  );
}

/** `[auth]` 编辑卡：白名单、会话与凭证、切片签名、限流与会话、登录防爆破。 */
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
  const configGroups = Array.isArray((config as { groups?: unknown }).groups)
    ? ((config as { groups: { id: string; name?: string; mode?: string }[] }).groups ?? [])
    : [];
  const passthroughGroups = configGroups.filter((group) => group.mode === 'passthrough');
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
      await putConfig(next);
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
    <div className="card security-card">
      <div className="row between">
        <div>
          <h2>访问控制</h2>
          <p className="muted">
            这些设置影响播放链接、限流和封禁。改完点「保存访问控制」。
          </p>
        </div>
        <button
          disabled={saving}
          title="保存访问控制设置"
          onClick={() => void save()}
        >
          {saving ? '保存中…' : '保存访问控制'}
        </button>
      </div>

      <h3>白名单</h3>
      <p className="muted">
        每行一个地址，支持精确 IP、IPv4 CIDR 与 <code className="mono">*</code>。
        名单内的地址不受封禁与并发会话上限限制；只在「IP 管控」开启时生效。
      </p>
      <Field label="白名单地址">
        <textarea
          rows={4}
          aria-label="白名单地址"
          title="名单内的地址豁免封禁与并发会话上限；只在 IP 管控开启时生效"
          placeholder="203.0.113.7&#10;10.0.0.0/8&#10;*"
          value={whitelist}
          onChange={(event) => setWhitelist(event.target.value)}
        />
      </Field>

      <h3>访问与链接</h3>
      <div className="fields">
        <Field label="IP 管控" tip="开启后启用按 IP 的限流、封禁与并发会话上限">
          <div className="field-control">
            <Select
              ariaLabel="IP 管控"
              value={auth.ip_guard_enabled === true ? 'on' : 'off'}
              options={[
                { value: 'off', label: '关闭' },
                { value: 'on', label: '开启' },
              ]}
              onChange={(next) => setKey('ip_guard_enabled', next === 'on')}
              compact
            />
          </div>
        </Field>
        <Field label="播放凭证" tip="开启后每个设备凭证只能播放自己的授权频道">
          <div className="field-control">
            <Select
              ariaLabel="播放凭证"
              value={auth.token_enabled === true ? 'on' : 'off'}
              options={[
                { value: 'off', label: '关闭' },
                { value: 'on', label: '开启' },
              ]}
              onChange={(next) => setKey('token_enabled', next === 'on')}
              compact
            />
          </div>
        </Field>
        <NumberField
          field={{ key: 'token_ttl_hours', label: '凭证有效期', unit: '小时' }}
          value={authNumber(auth, 'token_ttl_hours')}
          onChange={(next) => setKey('token_ttl_hours', Number(next) || 0)}
        />
        <Field
          label="导入随机 ID"
          tip="开启后导入 M3U 时忽略 tvg-id，为每个新频道生成随机 ID。只影响之后导入的频道"
        >
          <div className="field-control">
            <Select
              ariaLabel="导入随机 ID"
              value={auth.random_id_enabled === true ? 'on' : 'off'}
              options={[
                { value: 'off', label: '关闭' },
                { value: 'on', label: '开启' },
              ]}
              onChange={(next) => setKey('random_id_enabled', next === 'on')}
              compact
            />
          </div>
        </Field>
      </div>
      {auth.token_enabled === false ? (
        <div className="msg warn">
          播放凭证已关闭：任何拿到地址的人都能播放，而且无法按用户收回。{' '}
          <button className="ghost" onClick={() => setKey('token_enabled', true)}>
            开启凭证校验
          </button>
        </div>
      ) : (
        <p className="msg ok">播放凭证已开启。旧版的「开放」分组不再生成免验证的播放地址。</p>
      )}
      {passthroughGroups.length > 0 && (
        <div className="msg warn">
          有 {passthroughGroups.length} 个本地分组使用「原样直通」（
          {passthroughGroups
            .slice(0, 3)
            .map((group) => group.name || group.id)
            .join('、')}
          {passthroughGroups.length > 3 ? '…' : ''}
          ）。这些分组绕过本机和凭证检查。建议改为「改写地址」。
        </div>
      )}

      <h3>切片签名</h3>
      <p className="muted">
        开启后清单里的切片地址带时间窗签名。同一窗口内所有用户拿到相同地址，
        CDN 只缓存一份。关闭「播放凭证」时本项不生效。
        修改密钥会让当前窗口内的旧链接立即失效。
      </p>
      <div className="fields">
        <Field label="切片签名" tip="开启后切片地址带时间签名。关闭播放凭证时本项不生效">
          <div className="field-control">
            <Select
              ariaLabel="切片签名"
              value={auth.slice_sign_enabled === false ? 'off' : 'on'}
              options={[
                { value: 'on', label: '开启' },
                { value: 'off', label: '关闭' },
              ]}
              onChange={(next) => setKey('slice_sign_enabled', next === 'on')}
              compact
            />
          </div>
        </Field>
        <NumberField
          field={{ key: 'slice_sign_window_secs', label: '签名时间窗', unit: '秒', hint: '范围 30–3600' }}
          value={authNumber(auth, 'slice_sign_window_secs')}
          onChange={(next) => setKey('slice_sign_window_secs', Number(next) || 0)}
        />
        <Field
          label="签名密钥"
          tip="改动后当前窗口内的旧签名链接会立即失效"
          hint="十六进制，32–128 位。改动后需要保存。"
        >
          <div className="field-control">
            <div className="input-group">
              <input
                className="mono"
                aria-label="签名密钥"
                value={typeof auth.slice_sign_secret === 'string' ? auth.slice_sign_secret : ''}
                placeholder="留空自动生成 32 字节"
                onChange={(event) => setKey('slice_sign_secret', event.target.value.trim())}
              />
              <button
                type="button"
                title="生成一个新的十六进制密钥"
                onClick={() => setKey('slice_sign_secret', randomSecret())}
              >
                生成随机
              </button>
            </div>
          </div>
        </Field>
      </div>

      <h3>限流与会话</h3>
      <p className="muted">
        限流只统计播放入口请求（换台、新建播放），不计切片。二级是突发上限，
        一级是持续上限；任一超限即进入冷却，并记一次违规。全部只在「IP 管控」开启时生效。
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
        <Field label="限流跳转地址" tip="触发限流时跳转的地址">
          <div className="field-control">
            <input
              aria-label="限流跳转地址"
              value={typeof auth.limit_url === 'string' ? auth.limit_url : ''}
              onChange={(event) => setKey('limit_url', event.target.value)}
            />
          </div>
        </Field>
        <Field label="封禁跳转地址" tip="地址被封禁时跳转的地址">
          <div className="field-control">
            <input
              aria-label="封禁跳转地址"
              value={typeof auth.ban_url === 'string' ? auth.ban_url : ''}
              onChange={(event) => setKey('ban_url', event.target.value)}
            />
          </div>
        </Field>
      </div>

      <h3>登录防爆破</h3>
      <p className="muted">
        同一 IP 的登录失败数在统计窗口内达到「失败容忍次数 + 追加失败次数」后，
        登录接口 <code className="mono">/panel/api/login</code> 返回{' '}
        <code className="mono">429</code>；已登录的面板会话不受影响。
        首次锁定使用「首次锁定时长」；「IP 管控」开启时，后续锁定按阶梯加长。
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

/** 安全管控：访问控制、IP 统计与封禁。 */
export function SecurityPage() {
  const [stats, setStats] = useState<IpStats | null>(null);
  const [ip, setIp] = useState('');
  const [seconds, setSeconds] = useState('3600');
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  /** 访问控制保存后让只读检查卡重新读取。 */
  const [linkRefresh, setLinkRefresh] = useState(0);

  const load = useCallback(async () => {
    try {
      setStats(await api<IpStats>('GET', '/security/ip/stats'));
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

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
      <PageHead icon="security" title="安全管控" sub="访问控制、播放链接检查与 IP 封禁">
        <button
          className="ghost"
          title="重新读取 IP 统计"
          onClick={() => void load()}
        >
          <Icon name="refresh" size={14} />
          刷新
        </button>
      </PageHead>

      <AuthConfigCard
        onSaved={() => {
          void load();
          setLinkRefresh((next) => next + 1);
        }}
      />
      <LinkInfoCard refreshKey={linkRefresh} />

      <div className="card">
        <h2>IP 统计与封禁</h2>
        <p className="muted">
          共 {stats?.count ?? 0} 个地址，其中封禁 {stats?.banned ?? 0} 个。封禁只作用于非面板路径。
        </p>
        {message && <div className="msg ok">{message}</div>}
        {error && <div className="msg err">{error}</div>}
        <div className="row">
          <input
            aria-label="要封禁的地址"
            title="可一次填写多个地址，用逗号或换行分隔"
            placeholder="地址，可用逗号或换行分隔多个"
            value={ip}
            onChange={(event) => setIp(event.target.value)}
            style={{ flex: 2 }}
          />
          <input
            aria-label="封禁时长（秒）"
            title="留空使用默认封禁时长"
            placeholder="秒"
            value={seconds}
            onChange={(event) => setSeconds(event.target.value)}
            style={{ flex: 1 }}
          />
          <button
            title="按上面的地址与时长封禁"
            onClick={() => void ban()}
            disabled={ip.trim() === ''}
          >
            封禁
          </button>
          <button
            className="ghost"
            title="立即解除这些地址的封禁"
            onClick={() => void unbanBatch()}
            disabled={ip.trim() === ''}
          >
            批量解封
          </button>
          <Tip text="封禁只作用于非面板路径，不影响面板登录。" />
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
                    <button
                      className="ghost"
                      title="立即解除这个地址的封禁"
                      onClick={() => void unban(entry.ip)}
                    >
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

const LEVEL_LABELS: Record<Level, string> = {
  all: '全部级别',
  trace: '跟踪',
  debug: '调试',
  info: '信息',
  warn: '警告',
  error: '错误',
};

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
        sub={`最近 ${shown.length} 行${level === 'all' ? '' : `（${LEVEL_LABELS[level]}）`}（共 ${total} 行）`}
      >
        <div className="row log-tools">
          <Select
            ariaLabel="日志级别"
            title="只显示所选级别的日志行"
            value={level}
            options={LEVELS.map((item) => ({
              value: item,
              label: LEVEL_LABELS[item],
            }))}
            onChange={(next) => setLevel(next as Level)}
            width={124}
          />
          <input
            aria-label="读取行数"
            title="读取最近多少行日志"
            value={limit}
            onChange={(event) => setLimit(event.target.value)}
            inputMode="numeric"
            style={{ width: 84 }}
          />
          <button
            className="ghost"
            title="重新读取日志缓冲"
            onClick={() => void load()}
          >
            <Icon name="refresh" size={14} />
            刷新
          </button>
          <Tip text="级别与行数只影响本页显示，不改变服务端日志配置。" />
        </div>
      </PageHead>
      <div className="card">
        {error && <div className="msg err">{error}</div>}
        <pre className="log" style={{ marginTop: 0 }}>
          {shown.map((line, index) => {
            const lv = levelOf(line);
            return (
              <span key={index} className={lv === '' ? undefined : `lv-${lv}`}>
                {plain(line)}
                {'\n'}
              </span>
            );
          })}
        </pre>
        {level !== 'all' && shown.length === 0 && (
          <p className="muted">当前日志里没有 {LEVEL_LABELS[level]} 级别的行。</p>
        )}
      </div>
    </>
  );
}

/** 备份与恢复。`embedded` 用于设置页，不再渲染独立页头。
 *  `bare` 去掉嵌入页头，由外层分组标题承担。 */
export function BackupPage({
  embedded = false,
  bare = false,
  onRestored,
}: {
  embedded?: boolean;
  bare?: boolean;
  onRestored?: () => void;
}) {
  const [message, setMessage] = useState('');
  const [error, setError] = useState('');
  const [scope, setScope] = useState('all');
  const [pending, setPending] = useState<File | null>(null);
  const [busy, setBusy] = useState(false);
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

  const restore = async (selected: File) => {
    setBusy(true);
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
      setMessage(`已恢复 ${data.report?.files.length ?? 0} 个文件，配置已重新读取`);
      onRestored?.();
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
      if (file.current) file.current.value = '';
    }
  };

  return (
    <>
      {embedded ? (
        bare ? null : (
          <div className="embedded-head">
            <div>
              <h2>备份与恢复</h2>
              <span className="muted">导出与回滚配置、脚本与资源</span>
            </div>
          </div>
        )
      ) : (
        <PageHead icon="download" title="备份与恢复" sub="导出与回滚配置、脚本与资源" />
      )}
      <div className="card">
        <div className="section-title">导出</div>
        <div className="row">
          <Select
            ariaLabel="备份范围"
            title="选择备份包含的配置与资源"
            value={scope}
            options={[
              { value: 'all', label: '全部', desc: '配置、脚本和资源。' },
              { value: 'epg-scripts', label: '仅节目单脚本', desc: '只导出节目单采集脚本。' },
              { value: 'resolver', label: '仅解析脚本', desc: '只导出频道地址解析脚本。' },
            ]}
            onChange={setScope}
            width={190}
          />
          <button
            title="按所选范围下载备份包"
            onClick={() => void download()}
          >
            下载备份
          </button>
          <Tip text="备份包包含配置与所选资源。下载后在异地保存一份。" />
        </div>
        <div className="section-title" style={{ marginTop: 20 }}>
          恢复
        </div>
        <p className="muted">
          恢复会覆盖当前配置。旧配置会另存为备份文件，内容有问题时不会覆盖当前数据。
          恢复成功后配置立即重新读取。
        </p>
        <label className="ghost file-pick" title="选择 .tar.gz 备份包">
          <Icon name="upload" size={14} />
          {pending ? pending.name : '选择备份文件'}
          <input
            ref={file}
            type="file"
            accept=".gz,application/gzip"
            aria-label="选择备份包"
            onChange={(event) => {
              const selected = event.target.files?.[0];
              if (selected) setPending(selected);
            }}
          />
        </label>
        {message && <div className="msg ok">{message}</div>}
        {error && <div className="msg err">{error}</div>}
      </div>

      {pending ? (
        <Confirm
          title="恢复备份？"
          text={`备份包「${pending.name}」会覆盖当前配置与同名资源。旧配置会另存为备份文件。`}
          confirmText="恢复备份"
          busy={busy}
          onCancel={() => {
            setPending(null);
            if (file.current) file.current.value = '';
          }}
          onConfirm={() => {
            const selected = pending;
            setPending(null);
            void restore(selected);
          }}
        />
      ) : null}
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
        <h2>节目单翻译</h2>
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
      await putConfig(next);
      setNote('节目单翻译配置已保存');
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
          <h2>
            节目单翻译
            <Tip text="译文会写进节目单。保存后用于下一次翻译。" />
          </h2>
          <p className="muted">
            把节目名称和简介翻译成目标语言。译文会写进节目单。
          </p>
        </div>
        <div className="row">
          <button
            className="ghost"
            title="用已保存的配置探测 MTranServer"
            disabled={busy}
            onClick={() => void testMtran()}
          >
            测试连接
          </button>
          <button
            className="ghost"
            title="删除已生成的 EPG 译文缓存"
            disabled={busy}
            onClick={() => void clearCache()}
          >
            清空译文缓存
          </button>
          <button
            title="保存节目单翻译设置"
            disabled={busy}
            onClick={() => void save()}
          >
            {busy ? '处理中…' : '保存翻译设置'}
          </button>
        </div>
      </div>

      <div className="fields">
        <Field label="启用" tip="开启后翻译节目信息。">
          <Select
            ariaLabel="启用"
            value={translate.enabled === true ? 'on' : 'off'}
            options={[
              { value: 'off', label: '关闭' },
              { value: 'on', label: '开启' },
            ]}
            onChange={(next) => setKey('enabled', next === 'on')}
          />
        </Field>
        <Field label="翻译服务" tip="选择用哪个服务翻译。">
          <Select
            ariaLabel="翻译服务"
            value={text('engine') || 'mtran'}
            options={[
              { value: 'mtran', label: 'MTranServer', desc: '在本机或局域网自建。' },
              { value: 'google', label: 'Google', desc: '使用 Google 的在线翻译。' },
            ]}
            onChange={(next) => setKey('engine', next)}
          />
        </Field>
        <label className="field">
          <span>MTranServer 地址</span>
          <input
            placeholder="http://127.0.0.1:8989"
            value={text('mtran_url')}
            onChange={(event) => setKey('mtran_url', event.target.value)}
          />
        </label>
        <label className="field">
          <span>访问令牌（可选）</span>
          <input
            value={text('mtran_token')}
            onChange={(event) => setKey('mtran_token', event.target.value)}
          />
        </label>
        <label className="field">
          <span>每次翻译条数</span>
          <NumberBox
            min={1}
            max={500}
            value={num('mtran_batch')}
            onChange={(next) => setKey('mtran_batch', Number(next) || 0)}
          />
        </label>
        <label className="field">
          <span>每轮最多翻译条数</span>
          <NumberBox
            min={0}
            value={num('mtran_max_per_round')}
            onChange={(next) => setKey('mtran_max_per_round', Number(next) || 0)}
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
        <Field label="译文位置" tip="译文放在节目名上方还是下方">
          <Select
            ariaLabel="译文位置"
            value={translate.translation_first === true ? 'first' : 'second'}
            options={[
              { value: 'first', label: '译文在上（原文在下）' },
              { value: 'second', label: '原文在上' },
            ]}
            onChange={(next) => setKey('translation_first', next === 'first')}
          />
        </Field>
      </div>

      <div className="row">
        <button
          className="ghost"
          title="按当前翻译设置立即翻译已有节目"
          disabled={busy}
          onClick={() => void runNow()}
        >
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
      // `[epg]` 段在启动时读取，改 cron 也不会立刻换调度；界面上写明。
      await putConfig({ ...config, epg: { ...epg, cron: expr.trim() } });
      setSaved(expr.trim());
      setNote('采集节奏已保存；[epg] 段需重启后重新排程');
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
        cron 表达式（5 段：分 时 日 月 周），按{' '}
        <code className="mono">[server].timezone_offset_hours</code> 解释。默认{' '}
        <code className="mono">0 */4 * * *</code>。<code className="mono">[epg]</code>{' '}
        段在启动时读取，改动需重启才生效。
      </p>
      <div className="row">
        <input
          className="mono"
          aria-label="cron 表达式"
          title="5 段 cron：分 时 日 月 周。改动后需重启才重新排程"
          style={{ flex: 2 }}
          value={expr}
          onChange={(event) => setExpr(event.target.value)}
        />
        <button
          className="ghost"
          title="校验表达式并显示接下来的执行时间"
          onClick={() => void preview()}
        >
          预览
        </button>
        <button
          title="保存采集节奏；[epg] 段需重启后生效"
          disabled={busy || expr.trim() === saved}
          onClick={() => void save()}
        >
          {busy ? '保存中…' : '保存'}
        </button>
        <Tip text="采集节奏决定自动更新的频率。手动「立即采集」不受影响。" />
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

/** 节目指南：采集状态、采集节奏、翻译设置、频道对应关系与额外频道。 */
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
  const [tab, setTab] = useState<'bindings' | 'sources' | 'rules' | 'advanced'>('bindings');

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
        <button
          className="ghost"
          title="重新读取采集状态与映射"
          onClick={() => void load()}
        >
          <Icon name="refresh" size={14} />
          刷新状态
        </button>
        <button
          className="ghost"
          title="按当前映射重新生成 XMLTV"
          onClick={() => void regenerate()}
          disabled={busy}
        >
          重新生成
        </button>
        <button
          title="立即运行一次全部节目源采集"
          onClick={() => void refresh()}
          disabled={busy || !status?.enabled}
        >
          {busy ? '采集中…' : '立即采集'}
        </button>
      </PageHead>
      <div className="tabs" role="tablist">
        {(
          [
            ['bindings', '频道绑定'],
            ['sources', 'EPG 源'],
            ['rules', '关键字规则'],
            ['advanced', '高级'],
          ] as const
        ).map(([id, label]) => (
          <button
            key={id}
            role="tab"
            aria-selected={tab === id}
            className={tab === id ? 'tab active' : 'tab'}
            onClick={() => setTab(id)}
          >
            {label}
          </button>
        ))}
      </div>
      {tab === 'bindings' && <EpgBindingsTab />}
      {tab === 'sources' && <EpgSourcesTab />}
      {tab === 'rules' && <EpgRulesTab />}
      {tab === 'advanced' && (
        <>
      <div className="card">
        <h2>节目单状态</h2>
        <dl>
          <dt>节目源</dt>
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
        <h2>频道对应关系</h2>
        <div className="fields">
          <Field label="本地分组" tip="要匹配的本地分组。填分组 ID。">
            <input
              placeholder="gid"
              value={mapping.gid}
              onChange={(event) => setMapping({ ...mapping, gid: event.target.value })}
            />
          </Field>
          <Field label="本地频道" tip="要匹配的本地频道。填频道 ID。">
            <input
              placeholder="cid"
              value={mapping.cid}
              onChange={(event) => setMapping({ ...mapping, cid: event.target.value })}
            />
          </Field>
          <Field label="节目源" tip="节目源的标识。在节目源设置里查看。">
            <input
              placeholder="source_id"
              value={mapping.source_id}
              onChange={(event) => setMapping({ ...mapping, source_id: event.target.value })}
            />
          </Field>
          <Field label="源频道" tip="节目源里的频道标识。">
            <input
              placeholder="source_channel_id"
              value={mapping.source_channel_id}
              onChange={(event) =>
                setMapping({ ...mapping, source_channel_id: event.target.value })
              }
            />
          </Field>
        </div>
        <div className="row">
          <button title="保存这条频道与节目源的对应关系" onClick={() => void saveMapping()}>
            保存对应关系
          </button>
          <Tip text="对应关系决定本地频道显示哪个节目源的节目单。" />
        </div>
        <div className="table-wrap">
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
                  <button
                    className="icon-btn"
                    title="删除这条对应关系"
                    onClick={() => void removeMapping(key)}
                  >
                    清除
                  </button>
                </td>
              </tr>
            ))}
            {Object.keys(data.mappings).length === 0 && (
              <tr>
                <td colSpan={5} className="muted">
                  暂无对应关系。
                </td>
              </tr>
            )}
          </tbody>
        </table>
        </div>
      </div>

      <EpgMatchToolsCard onPick={(pick) => setMapping(pick)} />

      <div className="card">
        <h2>额外频道</h2>
        <div className="fields">
          <Field label="节目源" tip="节目源的标识。在节目源设置里查看。">
            <input
              placeholder="source_id"
              value={extra.source_id}
              onChange={(event) => setExtra({ ...extra, source_id: event.target.value })}
            />
          </Field>
          <Field label="源频道" tip="节目源里的频道标识。">
            <input
              placeholder="source_channel_id"
              value={extra.source_channel_id}
              onChange={(event) => setExtra({ ...extra, source_channel_id: event.target.value })}
            />
          </Field>
          <Field label="输出 ID" tip="留空时使用源频道 ID。">
            <input
              placeholder="out_id（可空）"
              value={extra.out_id}
              onChange={(event) => setExtra({ ...extra, out_id: event.target.value })}
            />
          </Field>
          <Field label="输出名称" tip="留空时使用源频道名称。">
            <input
              placeholder="out_name（可空）"
              value={extra.out_name}
              onChange={(event) => setExtra({ ...extra, out_name: event.target.value })}
            />
          </Field>
        </div>
        <div className="row">
          <button
            title="把源频道加入节目单，不需要本地频道"
            onClick={() => void addExtra()}
          >
            添加额外频道
          </button>
          <Tip text="额外频道只出现在公开节目单里，不生成播放地址。" />
        </div>
        <div className="table-wrap">
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
                  <button
                    className="icon-btn"
                    title="从节目单移除这个额外频道"
                    onClick={() => void removeExtra(item)}
                  >
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
      </div>
        </>
      )}
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
  const [pendingDelete, setPendingDelete] = useState<string | null>(null);

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
      <div className="table-wrap">
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
                    title="删除这个采集脚本"
                    onClick={() => setPendingDelete(script.name)}
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
      </div>
      <div className="row" style={{ marginTop: 8 }}>
        <input
          aria-label="脚本文件名"
          title="以 .py 结尾。同名文件会被覆盖"
          placeholder="脚本名，例如 mysource.py"
          value={name}
          onChange={(event) => setName(event.target.value)}
          style={{ flex: 1 }}
        />
        <button
          title="保存脚本文件；同名文件直接覆盖"
          onClick={() => void save()}
          disabled={name.trim() === ''}
        >
          上传/覆盖
        </button>
        <Tip text="脚本在服务端以 Python 运行。只上传可信脚本。" />
      </div>
      <textarea
        aria-label="脚本正文"
        title="Python 脚本正文。保存后按采集计划运行"
        placeholder="脚本正文"
        value={content}
        onChange={(event) => setContent(event.target.value)}
        rows={8}
        style={{ width: '100%', marginTop: 8 }}
      />
      {message && <div className="msg ok">{message}</div>}
      {error && <div className="msg err">{error}</div>}

      {pendingDelete ? (
        <Confirm
          title="删除采集脚本？"
          text={`脚本「${pendingDelete}」删除后，引用它的节目源会采集失败。`}
          confirmText="删除脚本"
          onCancel={() => setPendingDelete(null)}
          onConfirm={() => {
            const name = pendingDelete;
            setPendingDelete(null);
            void remove(name);
          }}
        />
      ) : null}
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
      <p className="muted">显示本地频道与节目源的匹配结果。</p>
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
        <h2>翻译服务状态</h2>
        <button className="ghost" onClick={() => void run()}>
          测试翻译服务
        </button>
      </div>
      <dl>
        <dt>启用</dt>
        <dd>{models.enabled ? '是' : '否'}</dd>
        <dt>服务</dt>
        <dd className="mono">
          {models.engine === 'mtran'
            ? 'MTranServer'
            : models.engine === 'google'
              ? 'Google'
              : '未配置'}
        </dd>
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
      <div className="fields">
        <Field label="本地分组" tip="要匹配的本地分组。填分组 ID。">
          <input
            placeholder="gid"
            value={gid}
            onChange={(event) => setGid(event.target.value)}
          />
        </Field>
        <Field label="本地频道" tip="要匹配的本地频道。填频道 ID。">
          <input
            placeholder="cid"
            value={cid}
            onChange={(event) => setCid(event.target.value)}
          />
        </Field>
      </div>
      <div className="row">
        <button
          className="ghost"
          title="按名称相似度查找可用的节目源频道"
          onClick={() => void runSuggest()}
          disabled={gid.trim() === '' || cid.trim() === ''}
        >
          匹配建议
        </button>
      </div>
      {suggestions.length > 0 && (
        <div className="table-wrap" style={{ marginTop: 8 }}>
        <table className="table" style={{ marginTop: 0 }}>
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
                    title="把这条建议填入对应关系表单"
                    onClick={() =>
                      onPick({
                        gid: gid.trim(),
                        cid: cid.trim(),
                        source_id: item.source_id,
                        source_channel_id: item.source_channel_id,
                      })
                    }
                  >
                    填入对应关系
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        </div>
      )}

      <div className="fields" style={{ marginTop: 12 }}>
        <Field label="节目源" tip="填节目源 ID。留空或填 __all__ 表示全部。">
          <input
            placeholder="source_id（__all__ = 全部）"
            value={sourceId}
            onChange={(event) => setSourceId(event.target.value)}
          />
        </Field>
        <Field label="搜索词" tip="按频道名过滤源频道。">
          <input
            placeholder="搜索词"
            value={query}
            onChange={(event) => setQuery(event.target.value)}
          />
        </Field>
      </div>
      <div className="row">
        <button
          className="ghost"
          title="列出节目源中的频道"
          onClick={() => void runSource()}
        >
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

      <div className="fields" style={{ marginTop: 12 }}>
        <Field label="时区偏移" tip="正数向后移，负数向前移，单位是分钟。">
          <input
            placeholder="偏移分钟"
            value={shift}
            onChange={(event) => setShift(event.target.value)}
          />
        </Field>
      </div>
      <div className="row">
        <button
          className="ghost"
          title="按偏移量预览节目时间"
          onClick={() => void runTz()}
        >
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
        <h2>公开节目单频道</h2>
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

type LinkInfo = {
  aliases: Record<string, unknown>;
  random_id_enabled: boolean;
  token: string;
  token_enabled: boolean;
  use_alias: boolean;
};

/** 播放链接检查：`GET /panel/api/link-info`。
 *
 * 只读：开关分布在「访问控制」与配置文件里，本卡只汇总当前结果。
 * 全局播放令牌已废弃，凭证按用户与设备下发（`docs/design/09-users-and-devices.md` §9.2）。
 */
function LinkInfoCard({ refreshKey }: { refreshKey: number }) {
  const [info, setInfo] = useState<LinkInfo | null>(null);
  const [error, setError] = useState('');

  const load = useCallback(async () => {
    try {
      setInfo(await api<LinkInfo>('GET', '/link-info'));
      setError('');
    } catch (err) {
      setError(errorText(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load, refreshKey]);

  const aliases = Object.keys(info?.aliases ?? {});

  return (
    <div className="card">
      <div className="row between">
        <div>
          <h2>
            播放链接检查 <Badge tone="muted">只读</Badge>
          </h2>
          <p className="muted">
            汇总播放链接用到的开关，用来核对。本卡不改配置：播放凭证与导入随机 ID
            在「访问控制」修改；用户与设备凭证在「用户与设备」分区管理。
          </p>
        </div>
        <button className="ghost" title="重新读取播放链接开关" onClick={() => void load()}>
          <Icon name="refresh" size={14} />
          刷新
        </button>
      </div>
      {info ? (
        <dl>
          <dt>播放凭证校验</dt>
          <dd>
            <Badge tone={info.token_enabled ? 'ok' : 'warn'}>
              {info.token_enabled ? '已开启' : '已关闭'}
            </Badge>
            <span className="muted"> 开启后，播放地址只使用「用户与设备」下发的凭证。</span>
          </dd>
          <dt>全局播放令牌</dt>
          <dd className="muted">已废弃。播放凭证按用户与设备下发。</dd>
          <dt>导入随机 ID</dt>
          <dd>
            <Badge tone={info.random_id_enabled ? 'ok' : 'muted'}>
              {info.random_id_enabled ? '已开启' : '已关闭'}
            </Badge>
            <span className="muted"> 开启后导入 M3U 时忽略 tvg-id，给新频道生成随机 ID。</span>
          </dd>
          <dt>链接别名</dt>
          <dd>
            {aliases.length > 0 ? (
              <span className="mono">{aliases.join(', ')}</span>
            ) : (
              <span className="muted">未设置</span>
            )}
            <span className="muted"> 别名只在配置文件的 [auth] aliases 中设置。</span>
          </dd>
        </dl>
      ) : error ? null : (
        <p className="muted">读取中…</p>
      )}
      {info && !info.token_enabled && (
        <div className="msg warn">播放凭证校验已关闭：拿到播放地址的人都能播放。</div>
      )}
      {error && <div className="msg err">{error}</div>}
    </div>
  );
}
