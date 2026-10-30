import { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { api, clearToken, getToken, setToken, UNAUTHORIZED_EVENT } from './api';
import { Icon, type IconName } from './icons';
import { PageHead, useToast } from './ui';
import { GroupsPage } from './groups';
import { EpgPanelPage, LogsPage, SecurityPage } from './ops';
import { CachePage, StatsPage } from './cache';
import { AssetsPage } from './assets';
import { SettingsPage } from './settings';
import { LogosPage } from './logo';
import { PlaylistPage } from './playlist';
import { LivestreamPage, SchedulePage } from './schedule';
import './style.css';

/** 侧栏分区；顺序按目标面板。 */
type SectionId =
  | 'dashboard'
  | 'cache'
  | 'groups'
  | 'settings'
  | 'security'
  | 'stats'
  | 'schedule'
  | 'epg'
  | 'logos'
  | 'subs'
  | 'livestream'
  | 'assets'
  | 'logs';

const NAV: { id: SectionId; label: string; icon: IconName }[] = [
  { id: 'dashboard', label: '概览', icon: 'overview' },
  { id: 'cache', label: '缓存状态', icon: 'cache' },
  { id: 'groups', label: '分组管理', icon: 'groups' },
  { id: 'settings', label: '系统设置', icon: 'settings' },
  { id: 'security', label: '安全管控', icon: 'security' },
  { id: 'stats', label: '频道热度', icon: 'stats' },
  { id: 'schedule', label: '定时录制', icon: 'schedule' },
  { id: 'epg', label: '节目指南', icon: 'epg' },
  { id: 'logos', label: '台标管理', icon: 'logo' },
  { id: 'subs', label: '订阅聚合', icon: 'subs' },
  { id: 'livestream', label: '回看管理', icon: 'livestream' },
  { id: 'assets', label: '视频素材', icon: 'assets' },
  { id: 'logs', label: '运行日志', icon: 'logs' },
];

type Status = Record<string, unknown>;

function Login({ onDone }: { onDone: () => void }) {
  const [username, setUsername] = useState('');
  const [password, setPassword] = useState('');
  const [error, setError] = useState('');
  const [busy, setBusy] = useState(false);

  const submit = async (event: React.FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError('');
    try {
      const data = await api<{ token: string }>('POST', '/login', { username, password });
      setToken(data.token);
      onDone();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="login-wrap">
      <div className="login-card">
        <div className="mark">
          <Icon name="tv" size={40} />
        </div>
        <div className="name">TV Buddy</div>
        <div className="tagline">Management Console</div>
        <form onSubmit={submit}>
          <label htmlFor="user">用户名</label>
          <input
            id="user"
            placeholder="用户名"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
          <label htmlFor="pass">密码</label>
          <input
            id="pass"
            type="password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
          />
          <button type="submit" disabled={busy}>
            {busy ? '登录中…' : '登 录'}
          </button>
        </form>
        {error && <div className="msg err">{error}</div>}
      </div>
    </div>
  );
}

/** 字节数转可读文本。 */
function bytes(value: number): string {
  if (!Number.isFinite(value) || value <= 0) return '0 B';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let size = value;
  let index = 0;
  while (size >= 1024 && index < units.length - 1) {
    size /= 1024;
    index += 1;
  }
  return `${size >= 100 || index === 0 ? Math.round(size) : size.toFixed(2)} ${units[index]}`;
}

/** 秒数转「N 小时 M 分钟」。 */
function uptime(secs: number): string {
  const total = Math.max(0, Math.floor(secs));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  if (hours > 0) return `${hours} 小时 ${minutes} 分钟`;
  return `${minutes} 分钟`;
}

/** 单个指标卡。`icon` 显示在标签左侧。 */
function Stat({
  label,
  value,
  tone,
  icon,
}: {
  label: string;
  value: string;
  tone?: string;
  icon?: IconName;
}) {
  return (
    <div className="stat">
      <div className="k">
        {icon && <Icon name={icon} size={13} />}
        {label}
      </div>
      <div className={`v${tone ? ` ${tone}` : ''}`}>{value}</div>
    </div>
  );
}

function Overview() {
  const [status, setStatus] = useState<Status | null>(null);
  const [error, setError] = useState('');
  const [busy, setBusy] = useState('');
  const { node, ok, err } = useToast();

  const load = useCallback(async () => {
    try {
      setStatus(await api<Status>('GET', '/status'));
      setError('');
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  /** 快速操作：端点都是现成的，成功后刷新状态。 */
  const action = async (name: string, path: string, done: string) => {
    setBusy(name);
    try {
      await api('POST', path);
      ok(done);
      await load();
    } catch (cause) {
      err(cause instanceof Error ? cause.message : String(cause));
    } finally {
      setBusy('');
    }
  };

  const num = (key: string): number => {
    const value = status?.[key];
    return typeof value === 'number' ? value : 0;
  };
  const text = (key: string): string => {
    const value = status?.[key];
    return value === undefined || value === null ? '—' : String(value);
  };

  const backend = text('cache_backend');

  return (
    <>
      <PageHead icon="overview" title="系统概览" sub="实时运行状态">
        <span className="badge ok">● 运行 {uptime(num('uptime_secs'))}</span>
      </PageHead>

      <div className="stats">
        <Stat label="分组数" value={text('group_count')} tone="accent" icon="groups" />
        <Stat label="频道数" value={text('channel_count')} tone="accent" icon="livestream" />
        <Stat
          label="缓存后端"
          value={backend}
          tone={backend === 'disk' ? 'danger' : 'accent'}
          icon="cache"
        />
        <Stat label="在线观众" value={text('online_viewers')} tone="accent" icon="stats" />
        <Stat
          label="缓存磁盘占用"
          value={`${text('disk_cache_mb')} MB`}
          tone="accent"
          icon="file"
        />
      </div>

      <div className="card">
        <div className="section-title">实时指标</div>
        <div className="stats">
          <Stat label="下行 · 回源" value={bytes(num('traffic_in_bytes'))} icon="download" />
          <Stat label="上行 · 出网" value={bytes(num('traffic_out_bytes'))} icon="upload" />
          <Stat label="进程内存" value={`${text('mem_rss_mb')} MB`} icon="cache" />
          <Stat label="CPU 占用" value={`${text('cpu_seconds')} 秒`} icon="stats" />
          <Stat
            label="整盘内存峰值"
            value={`${bytes(num('whole_file_peak'))} / ${bytes(num('whole_file_budget'))}`}
            icon="file"
          />
        </div>
      </div>

      <div className="card">
        <div className="section-title">快速操作</div>
        <div className="row">
          <button
            className="ghost"
            disabled={busy !== ''}
            onClick={() => void action('flush', '/cache/flush', '本地缓存已清空')}
          >
            <Icon name="trash" size={14} />
            {busy === 'flush' ? '清理中…' : '清空缓存'}
          </button>
          <button
            className="ghost"
            disabled={busy !== ''}
            onClick={() => void action('reload', '/reload', '配置已热重载')}
          >
            <Icon name="refresh" size={14} />
            {busy === 'reload' ? '重载中…' : '热重载配置'}
          </button>
          <button className="ghost" disabled={busy !== ''} onClick={() => void load()}>
            <Icon name="refresh" size={14} />
            刷新状态
          </button>
          <a className="ghost" href="/playlist.m3u" target="_blank" rel="noreferrer">
            <Icon name="download" size={14} />
            导出全部 M3U
          </a>
        </div>
      </div>

      <div className="card">
        <div className="section-title">系统信息</div>
        <dl>
          <div style={{ display: 'contents' }}>
            <dt>版本</dt>
            <dd>{text('version')}</dd>
          </div>
          <div style={{ display: 'contents' }}>
            <dt>运行时长</dt>
            <dd>{uptime(num('uptime_secs'))}</dd>
          </div>
          <div style={{ display: 'contents' }}>
            <dt>缓存后端</dt>
            <dd>{backend}</dd>
          </div>
          <div style={{ display: 'contents' }}>
            <dt>进程内存（RSS）</dt>
            <dd>{text('mem_rss_mb')} MB</dd>
          </div>
          <div style={{ display: 'contents' }}>
            <dt>缓存磁盘占用</dt>
            <dd>{text('disk_cache_mb')} MB</dd>
          </div>
          <div style={{ display: 'contents' }}>
            <dt>监听地址</dt>
            <dd className="mono">{text('bind')}</dd>
          </div>
        </dl>
      </div>

      {error && <div className="msg err">{error}</div>}
      {node}
    </>
  );
}

function Shell({ onLogout }: { onLogout: () => void }) {
  const [section, setSection] = useState<SectionId>('dashboard');
  /** 侧栏与页脚用的版本与运行时长。 */
  const [meta, setMeta] = useState<{ version: string; uptime: number } | null>(null);

  useEffect(() => {
    void api<{ version?: string; uptime_secs?: number }>('GET', '/status')
      .then((body) =>
        setMeta({
          version: String(body.version ?? '—'),
          uptime: Number(body.uptime_secs ?? 0),
        }),
      )
      .catch(() => setMeta(null));
  }, []);

  const logout = async () => {
    try {
      await api('POST', '/logout');
    } catch {
      // 忽略注销错误
    }
    clearToken();
    onLogout();
  };

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <Icon name="tv" size={24} className="brand-mark" />
          <div className="brand-text">
            <strong>TV Buddy</strong>
            <span>管理面板</span>
          </div>
        </div>
        <nav>
          {NAV.map((item) => (
            <button
              key={item.id}
              className={`nav-item${section === item.id ? ' active' : ''}`}
              onClick={() => setSection(item.id)}
            >
              <Icon name={item.icon} size={15} />
              {item.label}
            </button>
          ))}
        </nav>
        <div className="sidebar-meta">
          <div className="kv">
            <span>版本</span>
            <span className="v">{meta ? `v${meta.version}` : '—'}</span>
          </div>
          <div className="kv">
            <span>已运行</span>
            <span className="v">{meta ? uptime(meta.uptime) : '—'}</span>
          </div>
        </div>
        <div className="sidebar-foot">
          <button className="ghost" onClick={() => void logout()}>
            <Icon name="close" size={13} />
            退出登录
          </button>
        </div>
      </aside>
      <main className="content">
        {section === 'dashboard' && <Overview />}
        {section === 'cache' && <CachePage />}
        {section === 'groups' && <GroupsPage />}
        {section === 'settings' && <SettingsPage />}
        {section === 'security' && <SecurityPage />}
        {section === 'stats' && <StatsPage />}
        {section === 'schedule' && <SchedulePage view="schedule" />}
        {section === 'epg' && <EpgPanelPage />}
        {section === 'logos' && <LogosPage />}
        {section === 'subs' && <PlaylistPage />}
        {section === 'livestream' && <LivestreamPage />}
        {section === 'assets' && <AssetsPage />}
        {section === 'logs' && <LogsPage />}
        <footer className="app-foot">
          <Icon name="tv" size={13} />
          <span>TV Buddy</span>
          <span className="dot-sep">·</span>
          <span>IPTV 代理与管理面板</span>
          <span className="spacer" />
          <span className="mono">{meta ? `v${meta.version}` : ''}</span>
        </footer>
      </main>
    </div>
  );
}

function App() {
  const [loggedIn, setLoggedIn] = useState(() => getToken() !== '');

  // 会话失效（401）时回到登录卡。
  useEffect(() => {
    const onUnauthorized = () => setLoggedIn(false);
    window.addEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
    return () => window.removeEventListener(UNAUTHORIZED_EVENT, onUnauthorized);
  }, []);

  return loggedIn ? (
    <Shell onLogout={() => setLoggedIn(false)} />
  ) : (
    <Login onDone={() => setLoggedIn(true)} />
  );
}

createRoot(document.getElementById('root')!).render(<App />);
