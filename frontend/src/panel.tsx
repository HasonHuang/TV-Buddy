import { useEffect, useRef, useState } from 'react';
import { createRoot } from 'react-dom/client';

import { api, clearToken, getRole, getToken, setRole, setToken, UNAUTHORIZED_EVENT } from './api';
import { BrandMark, Icon, type IconName } from './icons';
import { Confirm, ThemeToggle } from './ui';
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
import { Overview, uptime } from './overview';
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
  // 节目指南常显，入口不随采集开关隐藏。开关只停采集，不停管理页。
  { id: 'epg', label: '节目指南', icon: 'epg', adminOnly: true },
  { feature: 'logo', id: 'logos', label: '台标管理', icon: 'logo', adminOnly: true },
  { feature: 'playlist', id: 'subs', label: '订阅源', icon: 'subs', adminOnly: true },
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

const SECTION_IDS = new Set<string>(NAV.map((item) => item.id));

/** 从地址栏哈希取分区。`#/groups` → `groups`，未知值返回 `null`。
 *
 * 面板挂在 `/panel`，没有前端路由库，也不会拦截服务端路径。用哈希做路由：
 * 切换分区只改片段，刷新不会把请求打到不存在的服务端路径。
 */
function sectionFromHash(hash = window.location.hash): SectionId | null {
  const id = hash.replace(/^#\/?/, '');
  return SECTION_IDS.has(id) ? (id as SectionId) : null;
}

/** 把分区写进地址栏。`replace` 用于首次落地与纠正非法分区，不新增历史记录。 */
function writeSection(next: SectionId, replace = false): void {
  const target = `#/${next}`;
  if (window.location.hash === target) return;
  if (replace) window.history.replaceState(null, '', target);
  else window.history.pushState(null, '', target);
}

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
    <div className="login">
      <div className="login-brand">
        <div className="rings" aria-hidden="true" />
        <div className="brandline">
          <span className="mark-tile" aria-hidden="true">
            <BrandMark size={24} />
          </span>
          <span>TV Buddy</span>
        </div>
        <div className="brand-copy">
          <h1>
            把频道，<span>留在自己手里。</span>
          </h1>
          <p>自托管的 IPTV 代理。缓存、转码、回看与订阅，都在本机。</p>
          <div className="login-mark" aria-hidden="true">
            <BrandMark size={92} />
          </div>
        </div>
        <div className="login-foot">SELF-HOSTED · 本地部署</div>
      </div>

      <div className="login-form-wrap">
        <ThemeToggle className="login-theme" showLabel />
        <form className="login-form" onSubmit={submit}>
          <h2>登录面板</h2>
          <div className="sub">使用管理员账号进入控制台。</div>
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
          {error && <div className="msg err">{error}</div>}
          <div className="hint">会话保存在本机浏览器。公共设备请在使用后退出。</div>
        </form>
      </div>
    </div>
  );
}

function Shell({ onLogout }: { onLogout: () => void }) {
  // 首屏分区取自地址栏，刷新与直接打开深链都能回原页。
  const [section, setSection] = useState<SectionId>(() => sectionFromHash() ?? 'dashboard');
  /** 当前分区。事件监听与浏览器前进/后退读它，避免闭包里拿到旧值。 */
  const sectionRef = useRef(section);
  /** 设置页有没有未保存修改；离开前先确认。只做拦截，不需要触发重渲染。 */
  const settingsDirtyRef = useRef(false);
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

  /** 切分区并同步地址栏。 */
  const show = (next: SectionId) => {
    sectionRef.current = next;
    setSection(next);
    writeSection(next);
  };

  /** 切分区。设置页有未保存修改时先弹确认。 */
  const go = (next: SectionId) => {
    // 点当前分区不触发确认，也不清未保存标记。
    if (next === sectionRef.current) return;
    if (sectionRef.current === 'settings' && settingsDirtyRef.current) {
      setPendingNav(next);
      return;
    }
    show(next);
  };

  /** 设置页报告未保存状态。地址栏监听与切页拦截都读这个 ref。 */
  const markSettingsDirty = (dirty: boolean) => {
    settingsDirtyRef.current = dirty;
  };

  // 浏览器前进/后退：地址已经变了，让面板跟上。设置页有未保存修改时先确认；
  // 取消（或非法分区）后把地址改回当前分区，避免面板与地址栏不一致。
  useEffect(() => {
    const onHistoryChange = () => {
      const next = sectionFromHash();
      if (!next) {
        writeSection(sectionRef.current, true);
        return;
      }
      if (next === sectionRef.current) return;
      if (sectionRef.current === 'settings' && settingsDirtyRef.current) {
        writeSection(sectionRef.current, true);
        setPendingNav(next);
        return;
      }
      sectionRef.current = next;
      setSection(next);
    };
    window.addEventListener('popstate', onHistoryChange);
    window.addEventListener('hashchange', onHistoryChange);
    return () => {
      window.removeEventListener('popstate', onHistoryChange);
      window.removeEventListener('hashchange', onHistoryChange);
    };
  }, []);

  useEffect(() => {
    const gotoMy = () => go('my');
    const gotoGroups = () => go('groups');
    const gotoStats = () => go('stats');
    const gotoSchedule = () => go('schedule');
    window.addEventListener('tv-buddy:goto-my', gotoMy);
    window.addEventListener('tv-buddy:goto-groups', gotoGroups);
    window.addEventListener('tv-buddy:goto-stats', gotoStats);
    window.addEventListener('tv-buddy:goto-schedule', gotoSchedule);
    return () => {
      window.removeEventListener('tv-buddy:goto-my', gotoMy);
      window.removeEventListener('tv-buddy:goto-groups', gotoGroups);
      window.removeEventListener('tv-buddy:goto-stats', gotoStats);
      window.removeEventListener('tv-buddy:goto-schedule', gotoSchedule);
    };
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

  // 地址栏始终指向实际分区：首屏补上哈希，非法或无权访问的深链回落到可用分区。
  useEffect(() => {
    sectionRef.current = active;
    if (active !== section) setSection(active);
    writeSection(active, true);
  }, [active, section]);

  const query = navQuery.trim().toLowerCase();
  const shownNav = query
    ? visibleNav.filter((item) => item.label.toLowerCase().includes(query))
    : visibleNav;

  // 管理员的「我的订阅」移出主菜单，放在侧栏底部，与后台分区分开。
  // 普通用户只有这一个分区，仍放在菜单顶部，避免侧栏只剩底部一条。
  const myItem = shownNav.find((item) => item.id === 'my');
  const bottomMy = isAdmin ? myItem : undefined;
  const navItems = bottomMy ? shownNav.filter((item) => item.id !== 'my') : shownNav;

  const navButton = (item: (typeof NAV)[number]) => (
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
  );

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
          {navItems.map(navButton)}
          {query && shownNav.length === 0 ? <div className="nav-empty">没有匹配的分区</div> : null}
        </nav>
        {bottomMy ? <div className="sidebar-my">{navButton(bottomMy)}</div> : null}
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
            <Icon name="close" size={14} />
            <span className="nav-label">退出登录</span>
          </button>
        </div>
      </aside>
      <main className="content">
        {active === 'my' && <MySubsPage />}
        {active === 'dashboard' && (
          <Overview features={features} featureLabels={FEATURE_LABELS} />
        )}
        {active === 'cache' && <CachePage />}
        {active === 'groups' && <GroupsPage />}
        {active === 'settings' && <SettingsPage onDirtyChange={markSettingsDirty} />}
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
            markSettingsDirty(false);
            show(next);
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

  // 日期与时间输入：点输入框任意位置都打开选择器，不再只认右侧小图标。
  // 只有 showPicker 与 WebKit 选择器图标都可用时才换成左侧图标；
  // 其它浏览器保留原生小图标作为退路。
  useEffect(() => {
    const supportsPicker = typeof HTMLInputElement.prototype.showPicker === 'function';
    const supportsIndicator =
      typeof CSS !== 'undefined' &&
      CSS.supports?.('selector(input::-webkit-calendar-picker-indicator)') === true;
    document.documentElement.classList.toggle(
      'picker-custom',
      supportsPicker && supportsIndicator,
    );
    return () => {
      document.documentElement.classList.remove('picker-custom');
    };
  }, []);

  return loggedIn ? (
    <Shell onLogout={() => setLoggedIn(false)} />
  ) : (
    <Login onDone={() => setLoggedIn(true)} />
  );
}

createRoot(document.getElementById('root')!).render(<App />);
