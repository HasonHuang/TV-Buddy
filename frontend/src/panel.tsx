import { useCallback, useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { api, clearToken, getRole, getToken, setRole, setToken, UNAUTHORIZED_EVENT } from './api';
import { BrandMark, Icon, type IconName } from './icons';
import { Confirm, PageHead, ThemeToggle, useToast } from './ui';
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

/** 搜索框里的快捷键提示：Mac 显示 ⌘K，其余显示 Ctrl K。 */
const SHORTCUT = /Macintosh|iPhone|iPad/.test(navigator.userAgent) ? '⌘K' : 'Ctrl K';

/** `[features]` 的开关名到分区名。概览用它在系统信息里列出已启用的模块。 */
const FEATURE_LABELS: [string, string][] = NAV.flatMap((item) =>
  item.feature ? [[item.feature, item.label] as [string, string]] : [],
);

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
      <ThemeToggle className="login-theme" showLabel />
      <div className="login-card">
        <div className="mark">
          <BrandMark size={44} />
        </div>
        <div className="name">TV Buddy</div>
        <div className="tagline">IPTV 代理与管理面板</div>
        <form onSubmit={submit}>
          <label htmlFor="user">用户名</label>
          <input
            id="user"
            autoComplete="username"
            placeholder="用户名"
            value={username}
            onChange={(e) => setUsername(e.target.value)}
          />
          <label htmlFor="pass">密码</label>
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

/** 单个指标卡。`icon` 显示在标签左侧，`unit` 用小一号字接在数值后。 */
function Stat({
  label,
  value,
  unit,
  tone,
  icon,
}: {
  label: string;
  value: string;
  unit?: string;
  tone?: string;
  icon?: IconName;
}) {
  return (
    <div className="stat">
      <div className="k">
        {icon && <Icon name={icon} size={13} />}
        {label}
      </div>
      <div className={`v${tone ? ` ${tone}` : ''}`}>
        {value}
        {unit ? <small>{unit}</small> : null}
      </div>
    </div>
  );
}

function Overview({ features }: { features: Record<string, boolean> | null }) {
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
  // 版本号在接口里不带前缀，界面统一显示 `v`。
  const version = status?.version ? `v${status.version}` : '—';
  // 与侧栏同一口径：缺少键也算启用，只有显式 false 才隐藏。
  const enabledFeatures = features
    ? FEATURE_LABELS.filter(([key]) => features[key] !== false)
    : [];

  return (
    <>
      <PageHead icon="overview" title="系统概览" sub="实时运行状态">
        <span className="badge ok">● 运行 {uptime(num('uptime_secs'))}</span>
      </PageHead>

      <div className="stats kpi">
        <Stat label="分组数" value={text('group_count')} icon="groups" />
        <Stat label="频道数" value={text('channel_count')} icon="livestream" />
        <Stat label="在线观众" value={text('online_viewers')} unit="人" icon="stats" />
        <Stat label="缓存磁盘占用" value={text('disk_cache_mb')} unit="MB" icon="file" />
      </div>

      <div className="dash-grid">
        <div className="dash-main">
          <section className="card">
            <div className="section-title">实时指标</div>
            <div className="metric-strip">
              <Stat label="下行 · 回源" value={bytes(num('traffic_in_bytes'))} icon="download" />
              <Stat label="上行 · 出网" value={bytes(num('traffic_out_bytes'))} icon="upload" />
              <Stat label="进程内存" value={text('mem_rss_mb')} unit="MB" icon="cache" />
              <Stat label="CPU 占用" value={text('cpu_seconds')} unit="秒" icon="stats" />
              <Stat
                label="整盘内存峰值"
                value={bytes(num('whole_file_peak'))}
                unit={`/ ${bytes(num('whole_file_budget'))}`}
                icon="file"
              />
            </div>
          </section>

          <section className="card">
            <div className="section-title">快速操作</div>
            <div className="quick-grid">
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
          </section>
        </div>

        <div className="dash-rail">
          <section className="card">
            <div className="section-title">系统信息</div>
            <div className="kv-list">
              <div className="kv">
                <span>版本</span>
                <b>{version}</b>
              </div>
              <div className="kv">
                <span>运行时长</span>
                <b>{uptime(num('uptime_secs'))}</b>
              </div>
              <div className="kv">
                <span>缓存后端</span>
                <b className="mono">{backend}</b>
              </div>
              <div className="kv">
                <span>监听地址</span>
                <b className="mono">{text('bind')}</b>
              </div>
            </div>
            <div className="feature-block">
              <div className="feature-title">已启用功能</div>
              <div className="feature-list">
                {enabledFeatures.length === 0 ? (
                  <span className="muted">{features ? '全部关闭' : '—'}</span>
                ) : (
                  enabledFeatures.map(([key, label], index) => (
                    <span key={key}>
                      {label}
                      {index < enabledFeatures.length - 1 ? (
                        <span className="sep"> · </span>
                      ) : null}
                    </span>
                  ))
                )}
              </div>
            </div>
          </section>
        </div>
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
  /** 侧栏分区搜索。只过滤导航，不触达接口。 */
  const [navQuery, setNavQuery] = useState('');
  const searchRef = useRef<HTMLInputElement>(null);
  /** 侧栏与页脚用的版本与运行时长。 */
  const [meta, setMeta] = useState<{ version: string; uptime: number } | null>(null);
  /** `[features]` 的开关键。为 `false` 的分区不显示入口。 */
  const [features, setFeatures] = useState<Record<string, boolean> | null>(null);
  /** 当前账号是否为管理员。服务端每次请求都会复核，这里只决定显示。 */
  const [isAdmin, setIsAdmin] = useState(getRole() === 'admin');

  // Cmd/Ctrl+K 聚焦分区搜索；Esc 清空并退出搜索。
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if ((event.metaKey || event.ctrlKey) && event.key.toLowerCase() === 'k') {
        event.preventDefault();
        searchRef.current?.focus();
        searchRef.current?.select();
        return;
      }
      if (event.key === 'Escape' && document.activeElement === searchRef.current) {
        setNavQuery('');
        searchRef.current?.blur();
      }
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, []);

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
  const query = navQuery.trim().toLowerCase();
  const shownNav = query
    ? visibleNav.filter((item) => item.label.toLowerCase().includes(query))
    : visibleNav;

  return (
    <div className="shell">
      <aside className="sidebar">
        <div className="brand">
          <BrandMark size={24} className="brand-mark" />
          <div className="brand-text">
            <strong>TV Buddy</strong>
            <span>管理面板</span>
          </div>
        </div>
        <label className="nav-search">
          <Icon name="search" size={13} />
          <input
            ref={searchRef}
            type="text"
            value={navQuery}
            placeholder="搜索分区"
            aria-label="搜索分区"
            autoComplete="off"
            spellCheck={false}
            onChange={(event) => setNavQuery(event.target.value)}
          />
          <kbd>{SHORTCUT}</kbd>
        </label>
        <nav>
          {shownNav.map((item) => (
            <button
              key={item.id}
              className={`nav-item${active === item.id ? ' active' : ''}`}
              title={item.label}
              aria-label={item.label}
              aria-current={active === item.id ? 'page' : undefined}
              onClick={() => {
                setNavQuery('');
                go(item.id);
              }}
            >
              <Icon name={item.icon} size={15} />
              <span className="nav-label">{item.label}</span>
            </button>
          ))}
          {query && shownNav.length === 0 ? <div className="nav-empty">没有匹配的分区</div> : null}
        </nav>
        {isAdmin && (
          <div className="sidebar-meta">
            <div className="kv">
              <span>版本</span>
              <span className="v" title={meta ? `v${meta.version}` : undefined}>
                {meta ? `v${meta.version}` : '—'}
              </span>
            </div>
            <div className="kv">
              <span>已运行</span>
              <span className="v">{meta ? uptime(meta.uptime) : '—'}</span>
            </div>
          </div>
        )}
        <div className="sidebar-foot">
          <ThemeToggle showLabel />
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
        {active === 'dashboard' && <Overview features={features} />}
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
          <BrandMark size={13} />
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
