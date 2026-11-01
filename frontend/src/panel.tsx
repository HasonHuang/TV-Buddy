import { useCallback, useEffect, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { api, clearToken, getRole, getToken, setRole, setToken, UNAUTHORIZED_EVENT } from './api';
import { Icon, type IconName } from './icons';
import { Confirm, PageHead, Tip, useToast } from './ui';
import { GroupsPage } from './groups';
import { EpgPanelPage, LogsPage, SecurityPage } from './ops';
import { CachePage, StatsPage } from './cache';
import { AssetsPage } from './assets';
import { SettingsPage } from './settings';
import { LogosPage } from './logo';
import { UsersPage } from './users';
import { PlaylistPage } from './playlist';
import { LivestreamPage, SchedulePage } from './schedule';
import { MySubsPage } from './my';
import './style.css';

/** 侧栏分区；顺序按目标面板。 */
type SectionId =
  | 'my'
  | 'dashboard'
  | 'cache'
  | 'groups'
  | 'settings'
  | 'security'
  | 'users'
  | 'stats'
  | 'schedule'
  | 'epg'
  | 'logos'
  | 'subs'
  | 'livestream'
  | 'assets'
  | 'logs';

/** `feature` 指向 `[features]` 的开关名。没有则该分区始终显示。
 *  `adminOnly` 的分区只给管理员；普通用户只看到「我的订阅」。 */
const NAV: { id: SectionId; label: string; icon: IconName; feature?: string; adminOnly?: boolean }[] = [
  { id: 'my', label: '我的订阅', icon: 'subs' },
  { id: 'dashboard', label: '概览', icon: 'overview', adminOnly: true },
  { id: 'cache', label: '缓存状态', icon: 'cache', adminOnly: true },
  { id: 'groups', label: '分组管理', icon: 'groups', adminOnly: true },
  { id: 'settings', label: '系统设置', icon: 'settings', adminOnly: true },
  { id: 'security', label: '安全管控', icon: 'security', adminOnly: true },
  { feature: 'users', id: 'users', label: '用户与设备', icon: 'users', adminOnly: true },
  { feature: 'stats', id: 'stats', label: '频道热度', icon: 'stats', adminOnly: true },
  { feature: 'recording', id: 'schedule', label: '定时录制', icon: 'schedule', adminOnly: true },
  { feature: 'epg', id: 'epg', label: '节目指南', icon: 'epg', adminOnly: true },
  { feature: 'logo', id: 'logos', label: '台标管理', icon: 'logo', adminOnly: true },
  { feature: 'playlist', id: 'subs', label: '订阅聚合', icon: 'subs', adminOnly: true },
  { feature: 'livestream', id: 'livestream', label: '回看管理', icon: 'livestream', adminOnly: true },
  { feature: 'assets', id: 'assets', label: '视频素材', icon: 'assets', adminOnly: true },
  { id: 'logs', label: '运行日志', icon: 'logs', adminOnly: true },
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
      const data = await api<{ token: string; role?: string }>('POST', '/login', {
        username,
        password,
      });
      setToken(data.token);
      setRole(data.role === 'admin' ? 'admin' : 'user');
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
          <label htmlFor="user">
            用户名
            <Tip text="面板账号。首次安装的账号与初始口令在安装输出中。" />
          </label>
          <input
            id="user"
            autoComplete="username"
            placeholder="用户名"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
          <label htmlFor="pass">
            密码
            <Tip text="登录口令。连续失败会触发临时锁定。" />
          </label>
          <input
            id="pass"
            type="password"
            autoComplete="current-password"
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
            title="删除已落盘的清单与切片，释放磁盘"
            disabled={busy !== ''}
            onClick={() => void action('flush', '/cache/flush', '本地缓存已清空')}
          >
            <Icon name="trash" size={14} />
            {busy === 'flush' ? '清理中…' : '清空缓存'}
          </button>
          <button
            className="ghost"
            title="从数据库重新读取配置并立即生效"
            disabled={busy !== ''}
            onClick={() => void action('reload', '/reload', '配置已热重载')}
          >
            <Icon name="refresh" size={14} />
            {busy === 'reload' ? '重载中…' : '热重载配置'}
          </button>
          <button
            className="ghost"
            title="重新读取运行状态与实时指标"
            disabled={busy !== ''}
            onClick={() => void load()}
          >
            <Icon name="refresh" size={14} />
            刷新状态
          </button>
          <button
            className="ghost"
            title="跳到我的订阅页，查看并复制订阅地址"
            onClick={() => window.dispatchEvent(new CustomEvent('tv-buddy:goto-my'))}
          >
            <Icon name="download" size={14} />
            我的订阅地址
          </button>
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
  /** 设置页有没有未保存修改；离开前先确认。 */
  const [settingsDirty, setSettingsDirty] = useState(false);
  const [pendingNav, setPendingNav] = useState<SectionId | null>(null);
  /** 侧栏与页脚用的版本与运行时长。 */
  const [meta, setMeta] = useState<{ version: string; uptime: number } | null>(null);
  /** `[features]` 的开关键。为 `false` 的分区不显示入口。 */
  const [features, setFeatures] = useState<Record<string, boolean> | null>(null);
  /** 当前账号是否为管理员。服务端每次请求都会复核，这里只决定显示。 */
  const [isAdmin, setIsAdmin] = useState(getRole() === 'admin');

  useEffect(() => {
    void api<{ user?: { role?: string } }>('GET', '/me')
      .then((body) => {
        const role = body.user?.role === 'admin' ? 'admin' : 'user';
        setRole(role);
        setIsAdmin(role === 'admin');
      })
      .catch(() => setIsAdmin(false));
  }, []);

  useEffect(() => {
    // 普通用户没有 `/status` 权限，跳过。
    if (!isAdmin) return;
    void api<{
      version?: string;
      uptime_secs?: number;
      features?: Record<string, boolean>;
    }>('GET', '/status')
      .then((body) => {
        setMeta({
          version: String(body.version ?? '—'),
          uptime: Number(body.uptime_secs ?? 0),
        });
        setFeatures(body.features ?? {});
      })
      .catch(() => setMeta(null));
  }, [isAdmin]);

  /** 切分区。设置页有未保存修改时先弹确认。 */
  const go = (next: SectionId) => {
    if (section === 'settings' && settingsDirty) {
      setPendingNav(next);
      return;
    }
    setSection(next);
  };

  useEffect(() => {
    const gotoMy = () => go('my');
    window.addEventListener('tv-buddy:goto-my', gotoMy);
    return () => window.removeEventListener('tv-buddy:goto-my', gotoMy);
  });

  const logout = async () => {
    try {
      await api('POST', '/logout');
    } catch {
      // 忽略注销错误
    }
    clearToken();
    onLogout();
  };

  // 普通用户只有「我的订阅」。关掉的功能分区也不显示入口。
  const visibleNav = NAV.filter((item) => {
    if (item.adminOnly && !isAdmin) return false;
    return !item.feature || features?.[item.feature] !== false;
  });
  const fallback: SectionId = isAdmin ? 'dashboard' : 'my';
  const active = visibleNav.some((item) => item.id === section) ? section : fallback;

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
          {visibleNav.map((item) => (
            <button
              key={item.id}
              className={`nav-item${active === item.id ? ' active' : ''}`}
              title={item.label}
              aria-label={item.label}
              aria-current={active === item.id ? 'page' : undefined}
              onClick={() => go(item.id)}
            >
              <Icon name={item.icon} size={15} />
              <span className="nav-label">{item.label}</span>
            </button>
          ))}
        </nav>
        {isAdmin && (
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
        )}
        <div className="sidebar-foot">
          <button
            className="ghost"
            title="结束当前面板会话并回到登录页"
            onClick={() => void logout()}
          >
            <Icon name="close" size={13} />
            <span className="nav-label">退出登录</span>
          </button>
        </div>
      </aside>
      <main className="content">
        {active === 'my' && <MySubsPage />}
        {active === 'dashboard' && <Overview />}
        {active === 'cache' && <CachePage />}
        {active === 'groups' && <GroupsPage />}
        {active === 'settings' && <SettingsPage onDirtyChange={setSettingsDirty} />}
        {active === 'security' && <SecurityPage />}
        {active === 'users' && <UsersPage />}
        {active === 'stats' && <StatsPage />}
        {active === 'schedule' && <SchedulePage view="schedule" />}
        {active === 'epg' && <EpgPanelPage />}
        {active === 'logos' && <LogosPage />}
        {active === 'subs' && <PlaylistPage />}
        {active === 'livestream' && <LivestreamPage />}
        {active === 'assets' && <AssetsPage />}
        {active === 'logs' && <LogsPage />}
        <footer className="app-foot">
          <Icon name="tv" size={13} />
          <span>TV Buddy</span>
          <span className="dot-sep">·</span>
          <span>IPTV 代理与管理面板</span>
          <span className="spacer" />
          <span className="mono">{meta ? `v${meta.version}` : ''}</span>
        </footer>
      </main>

      {pendingNav ? (
        <Confirm
          title="离开系统设置？"
          text="当前有未保存的修改。离开后这些修改会丢失。"
          confirmText="放弃修改并离开"
          onCancel={() => setPendingNav(null)}
          onConfirm={() => {
            const next = pendingNav;
            setPendingNav(null);
            setSettingsDirty(false);
            setSection(next);
          }}
        />
      ) : null}
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
