/** 系统设置：字段级表单。
 *
 * 一次 `GET /panel/api/config` 取整份配置到草稿，改完用顶部「保存设置」一次
 * `PUT` 回去（对齐目标 p12：修改后点击保存生效）。未覆盖的段保留最下面的
 * 「其余字段（JSON）」兜底编辑。
 *
 * 布局基线：`docs/design/reference/img/p12-settings.png`。
 */

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useState,
} from 'react';

import { api } from './api';
import { Icon } from './icons';
import { RecommendCard, type Recommend } from './lowcfg';
import { BackupPage } from './ops';
import { sha256Hex } from './sha256';
import { SystemPage } from './system';
import { PageHead, useToast } from './ui';

type Cfg = Record<string, unknown>;

/** 按 `a.b.c` 取子值。 */
function getPath(root: Cfg, path: string): unknown {
  return path.split('.').reduce<unknown>((acc, key) => {
    if (acc && typeof acc === 'object') return (acc as Cfg)[key];
    return undefined;
  }, root);
}

/** 按 `a.b.c` 不可变写子值。 */
function setPath(root: Cfg, path: string, value: unknown): Cfg {
  const [head, ...rest] = path.split('.');
  if (rest.length === 0) return { ...root, [head]: value };
  const child = (root[head] ?? {}) as Cfg;
  return { ...root, [head]: setPath(child, rest.join('.'), value) };
}

function asNumber(root: Cfg, path: string): number {
  const value = getPath(root, path);
  if (typeof value === 'number' && Number.isFinite(value)) return value;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : 0;
}

function asText(root: Cfg, path: string): string {
  const value = getPath(root, path);
  if (value === null || value === undefined) return '';
  return typeof value === 'string' ? value : String(value);
}

function asBool(root: Cfg, path: string): boolean {
  return getPath(root, path) === true;
}

function asList(root: Cfg, path: string): string[] {
  const value = getPath(root, path);
  return Array.isArray(value) ? value.map((item) => String(item)) : [];
}

type DraftValue = { draft: Cfg; update: (path: string, value: unknown) => void };

const DraftCtx = createContext<DraftValue | null>(null);

function useDraft(): DraftValue {
  const value = useContext(DraftCtx);
  if (!value) throw new Error('DraftCtx 未提供');
  return value;
}

/** 数字输入。 */
function Num({
  label,
  path,
  hint,
  unit,
  step,
}: {
  label: string;
  path: string;
  hint?: string;
  unit?: string;
  step?: number;
}) {
  const { draft, update } = useDraft();
  return (
    <div className="field">
      <label>
        {label}
        {unit ? <span className="muted">（{unit}）</span> : null}
      </label>
      <input
        type="number"
        min={0}
        step={step ?? 1}
        value={String(asNumber(draft, path))}
        onChange={(event) => {
          const next = Number(event.target.value);
          update(path, Number.isFinite(next) ? next : 0);
        }}
      />
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 文本输入。 */
function Text({
  label,
  path,
  hint,
  placeholder,
}: {
  label: string;
  path: string;
  hint?: string;
  placeholder?: string;
}) {
  const { draft, update } = useDraft();
  return (
    <div className="field">
      <label>{label}</label>
      <input
        value={asText(draft, path)}
        placeholder={placeholder}
        onChange={(event) => update(path, event.target.value)}
      />
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 下拉选择。 */
function Pick({
  label,
  path,
  hint,
  options,
}: {
  label: string;
  path: string;
  hint?: string;
  options: { value: string; label: string }[];
}) {
  const { draft, update } = useDraft();
  return (
    <div className="field">
      <label>{label}</label>
      <select value={asText(draft, path)} onChange={(event) => update(path, event.target.value)}>
        {options.map((option) => (
          <option key={option.value} value={option.value}>
            {option.label}
          </option>
        ))}
      </select>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 布尔下拉：值仍是布尔，只是外观与目标一致。 */
function BoolPick({
  label,
  path,
  hint,
  onLabel = '开启',
  offLabel = '关闭',
}: {
  label: string;
  path: string;
  hint?: string;
  onLabel?: string;
  offLabel?: string;
}) {
  const { draft, update } = useDraft();
  return (
    <div className="field">
      <label>{label}</label>
      <select
        value={asBool(draft, path) ? 'on' : 'off'}
        onChange={(event) => update(path, event.target.value === 'on')}
      >
        <option value="on">{onLabel}</option>
        <option value="off">{offLabel}</option>
      </select>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 复选框。 */
function Toggle({ label, path, hint }: { label: string; path: string; hint?: string }) {
  const { draft, update } = useDraft();
  return (
    <div className="field">
      <label className="check">
        <input
          type="checkbox"
          checked={asBool(draft, path)}
          onChange={(event) => update(path, event.target.checked)}
        />
        {label}
      </label>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 多选复选框（伪装格式）。 */
function Multi({
  label,
  path,
  hint,
  options,
}: {
  label: string;
  path: string;
  hint?: string;
  options: string[];
}) {
  const { draft, update } = useDraft();
  const chosen = asList(draft, path);
  return (
    <div className="field">
      <label>{label}</label>
      <div className="row">
        {options.map((option) => (
          <label key={option} className="check">
            <input
              type="checkbox"
              checked={chosen.includes(option)}
              onChange={(event) =>
                update(
                  path,
                  event.target.checked
                    ? [...chosen, option]
                    : chosen.filter((item) => item !== option),
                )
              }
            />
            {option}
          </label>
        ))}
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 多行文本 ⇄ 字符串数组（一行一条）。
 *
 * 编辑期间保留原文本（否则回车会被「去空行」吃掉），失焦时才写回草稿并丢掉空行。
 */
function Lines({
  label,
  path,
  hint,
  rows,
  placeholder,
}: {
  label: string;
  path: string;
  hint?: string;
  rows?: number;
  placeholder?: string;
}) {
  const { draft, update } = useDraft();
  const stored = asList(draft, path).join('\n');
  const [text, setText] = useState(stored);
  const [editing, setEditing] = useState(false);

  useEffect(() => {
    if (!editing) setText(stored);
  }, [stored, editing]);

  return (
    <div className="field">
      <label>{label}</label>
      <textarea
        rows={rows ?? 3}
        spellCheck={false}
        placeholder={placeholder}
        value={text}
        onFocus={() => setEditing(true)}
        onChange={(event) => setText(event.target.value)}
        onBlur={() => {
          setEditing(false);
          update(
            path,
            text
              .split('\n')
              .map((line) => line.trimEnd())
              .filter((line) => line !== ''),
          );
        }}
      />
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 标签多选（字幕目标语言）。选中集写回数组；`primaryPath` 同步首个选中项。 */
function Chips({
  label,
  path,
  primaryPath,
  hint,
  options,
}: {
  label: string;
  path: string;
  primaryPath?: string;
  hint?: string;
  options: { value: string; label: string }[];
}) {
  const { draft, update } = useDraft();
  const chosen = asList(draft, path);
  const toggle = (value: string) => {
    const next = chosen.includes(value)
      ? chosen.filter((item) => item !== value)
      : [...chosen, value];
    update(path, next);
    if (primaryPath) update(primaryPath, next[0] ?? '');
  };
  return (
    <div className="field">
      <label>{label}</label>
      <div className="row" style={{ flexWrap: 'wrap' }}>
        {options.map((option) => (
          <button
            key={option.value}
            type="button"
            className={chosen.includes(option.value) ? '' : 'ghost'}
            onClick={() => toggle(option.value)}
          >
            {option.label}
          </button>
        ))}
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

/** 滑杆（字幕位置）。 */
function Range({
  label,
  path,
  min,
  max,
  unit,
  hint,
}: {
  label: string;
  path: string;
  min: number;
  max: number;
  unit?: string;
  hint?: string;
}) {
  const { draft, update } = useDraft();
  const value = asNumber(draft, path);
  return (
    <div className="field">
      <label>{label}</label>
      <div className="row">
        <input
          type="range"
          min={min}
          max={max}
          value={value}
          onChange={(event) => update(path, Number(event.target.value))}
        />
        <span className="muted">
          {value}
          {unit ?? ''}
        </span>
      </div>
      {hint ? <div className="hint">{hint}</div> : null}
    </div>
  );
}

function Card({
  title,
  note,
  actions,
  children,
}: {
  title: string;
  note?: string;
  actions?: React.ReactNode;
  children: React.ReactNode;
}) {
  return (
    <div className="card">
      <div className="row between">
        <div>
          <h2>{title}</h2>
          {note ? <span className="muted">{note}</span> : null}
        </div>
        {actions}
      </div>
      {children}
    </div>
  );
}

const LOG_LEVELS = ['trace', 'debug', 'info', 'warn', 'error'];

const CATCHUP_TEMPLATES = [
  { value: 'guonei', label: '国产（DIYP / 影音仓库 / TVBox / OK）' },
  { value: 'dollar', label: '国外（${start}-${end}）' },
  { value: 'unix', label: 'Kodi（{utc}-{utcend}）' },
  { value: 'both', label: '两者都带' },
];

/** 字幕字号预设；`[subtitle].style.font_size` 存字符串百分比。 */
const FONT_SIZES = ['60%', '70%', '80%', '90%', '100%', '110%', '120%', '150%'].map((size) => ({
  value: size,
  label: size === '80%' ? '80%（默认）' : size,
}));

/** 字幕颜色；空串 = 跟随播放器默认。取值为十六进制，目标的具体预设未观测。 */
const SUBTITLE_COLORS = [
  { value: '', label: '默认颜色' },
  { value: '#ffffff', label: '白色' },
  { value: '#000000', label: '黑色' },
  { value: '#ffff00', label: '黄色' },
  { value: '#00ffff', label: '青色' },
  { value: '#ff0000', label: '红色' },
];

/** 字幕机翻的目标语言（按 p12 参考截图的顺序）。 */
const SUBTITLE_LANGS = [
  { value: 'zh-cn', label: '简体中文' },
  { value: 'zh-tw', label: '繁體中文' },
  { value: 'ja', label: '日本語' },
  { value: 'ko', label: '한국어' },
  { value: 'en', label: 'English' },
  { value: 'es', label: 'Español' },
  { value: 'fr', label: 'Français' },
  { value: 'de', label: 'Deutsch' },
  { value: 'ru', label: 'Русский' },
  { value: 'he', label: 'עברית' },
  { value: 'vi', label: 'Tiếng Việt' },
];

/** 系统设置页。 */
export function SettingsPage() {
  const [draft, setDraft] = useState<Cfg | null>(null);
  const [recommend, setRecommend] = useState<Recommend | null>(null);
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState('');
  const [dirty, setDirty] = useState(false);
  const { node, ok, err } = useToast();

  const load = useCallback(async () => {
    try {
      const next = await api<Cfg>('GET', '/config');
      setDraft(next);
      setPassword('');
      setDirty(false);
      try {
        const stats = await api<{ recommend: Recommend }>('GET', '/cache/stats');
        setRecommend(stats.recommend);
      } catch {
        setRecommend(null);
      }
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    }
  }, [err]);

  useEffect(() => {
    void load();
  }, [load]);

  const update = useCallback((path: string, value: unknown) => {
    setDraft((prev) => (prev ? setPath(prev, path, value) : prev));
    setDirty(true);
  }, []);

  const value = useMemo<DraftValue | null>(
    () => (draft ? { draft, update } : null),
    [draft, update],
  );

  const save = async () => {
    if (!draft) return;
    setBusy('save');
    try {
      const next =
        password.trim() === ''
          ? draft
          : setPath(draft, 'panel.password_sha256', sha256Hex(password));
      await api('PUT', '/config', next);
      setDraft(next);
      setPassword('');
      setDirty(false);
      ok('设置已保存');
      try {
        const stats = await api<{ recommend: Recommend }>('GET', '/cache/stats');
        setRecommend(stats.recommend);
      } catch {
        // 推荐值取不到不影响保存结果
      }
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const estimate = async () => {
    setBusy('estimate');
    try {
      const result = await api<{ groups: number; failed: { gid: string; cid: string }[] }>(
        'POST',
        '/cdn/estimate',
      );
      const failed = result.failed.length;
      ok(`估算完成：${result.groups} 个分组${failed ? `，失败 ${failed} 个` : ''}`);
      const stats = await api<{ recommend: Recommend }>('GET', '/cache/stats');
      setRecommend(stats.recommend);
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const flush = async () => {
    setBusy('flush');
    try {
      await api('POST', '/cache/flush');
      ok('本地缓存已清空');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const clearTranslated = async () => {
    setBusy('subclear');
    try {
      await api('POST', '/subtitle/translate/clear');
      ok('译文缓存已清空');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const testMtran = async () => {
    setBusy('mtran');
    try {
      const result = await api<{ ok: boolean; base?: string; targets?: { error?: string }[] }>(
        'POST',
        '/subtitle/mtran/test',
        { text: 'hello' },
      );
      if (result.ok) {
        ok(`机翻连通正常${result.base ? `（${result.base}）` : ''}`);
      } else {
        err(result.targets?.[0]?.error || '机翻测试失败');
      }
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy('');
    }
  };

  const ttlHint =
    asNumber(draft ?? {}, 'cdn.segment_max_age_secs') === 0
      ? `自动（当前 ${recommend?.ttl_secs ?? '—'}s）`
      : '手动值，覆盖自动推荐';
  const capHint =
    asNumber(draft ?? {}, 'cache.gc.max_disk_mb') === 0
      ? `自动（当前 ${recommend?.disk_cap_mb ?? '—'} MB）`
      : '手动值，覆盖自动推荐';

  return (
    <>
      <PageHead
        icon="settings"
        title="系统设置"
        sub="修改后点击保存生效；保存写回整份 config.toml。"
      >
        <button className="ghost" onClick={() => void load()}>
          <Icon name="refresh" size={14} />
          重新读取
        </button>
        <button disabled={busy === 'save' || !draft} onClick={() => void save()}>
          {busy === 'save' ? '保存中…' : dirty ? '保存设置 *' : '保存设置'}
        </button>
      </PageHead>

      {!draft || !value ? (
        <div className="card">
          <p className="muted">读取配置中…</p>
        </div>
      ) : (
        <DraftCtx.Provider value={value}>
          <div
            style={{
              display: 'grid',
              gridTemplateColumns: 'repeat(auto-fit, minmax(380px, 1fr))',
              gap: 12,
              alignItems: 'start',
            }}
          >
            <Card title="服务器" note="监听、跳转与订阅头">
              <div className="fields">
                <Text label="监听地址" path="server.bind" hint="改后需重启" />
                <Num label="时区" path="server.timezone_offset_hours" unit="小时" />
                <Pick label="回看格式" path="server.catchup_tpl" options={CATCHUP_TEMPLATES} />
                <Pick
                  label="日志级别"
                  path="server.log_level"
                  options={LOG_LEVELS.map((level) => ({ value: level, label: level }))}
                />
                <Text label="首页跳转" path="server.home_url" />
                <Text label="404 跳转" path="server.not_found_url" />
                <Text label="错误跳转" path="server.error_url" />
              </div>
              <Lines
                label="M3U 头部"
                path="server.m3u_header_lines"
                hint="一行一条，下发订阅时插在最前"
                rows={3}
                placeholder="#EXT-X-APTV-PREVIEW: FALSE"
              />
            </Card>

            <Card title="面板认证" note="面板登录口令">
              <Text label="用户名" path="panel.username" />
              <div className="field">
                <label>新密码</label>
                <input
                  type="password"
                  value={password}
                  placeholder="留空不修改"
                  onChange={(event) => setPassword(event.target.value)}
                />
                <div className="hint">留空不修改；填写后在浏览器里算 SHA-256 再写入配置</div>
              </div>
            </Card>

            <Card title="切片伪装" note="把切片伪装成图片，降低被举报风险">
              <div className="fields">
                <BoolPick label="切片伪装" path="cache.disguise.enabled" />
                <BoolPick label="字幕 .vtt" path="cache.disguise.subtitle" hint="推荐关闭" />
                <BoolPick label="列表伪装" path="cache.disguise.playlist" hint="推荐开启" />
                <BoolPick label="主列表" path="cache.disguise.master" hint="推荐关闭" />
              </div>
              <Multi
                label="伪装格式"
                path="cache.disguise.formats"
                hint="多选，按切片轮换；关闭伪装时下发 .ts"
                options={['jpg', 'png', 'gif', 'bmp', 'webp']}
              />
            </Card>

            <Card title="监控" note="分组页状态点与在线人数">
              <div className="fields">
                <BoolPick label="健康探测" path="health.enabled" />
                <Num label="探测周期" path="health.interval_secs" unit="秒" />
                <Num label="探测超时" path="health.timeout_secs" unit="秒" />
                <Num label="探测并发" path="health.concurrency" unit="个" />
                <BoolPick label="访问统计" path="stats.enabled" />
                <Num
                  label="在线窗口"
                  path="stats.online_window_secs"
                  unit="秒"
                  hint="窗口内的播放 token 计为在线"
                />
              </div>
            </Card>

            <Card
              title="缓存配置"
              note="后端与容量"
              actions={
                <button className="ghost" disabled={busy === 'flush'} onClick={() => void flush()}>
                  {busy === 'flush' ? '清理中…' : '清空本地缓存'}
                </button>
              }
            >
              <div className="fields">
                <Pick
                  label="后端"
                  path="cache.backend"
                  options={[
                    { value: 'disk', label: '硬盘（disk）' },
                    { value: 'memory', label: '内存（memory，重启清空）' },
                  ]}
                />
                <Text label="缓存目录" path="cache.disk_path" hint="相对进程工作目录；改后需重启" />
                <Num
                  label="硬盘上限"
                  path="cache.disk_max_mb"
                  unit="MB"
                  hint="0 = 不限；与「磁盘回收」的上限同义，同时非 0 取更小者"
                />
                <Num
                  label="内存上限"
                  path="cache.memory_max_mb"
                  unit="MB"
                  hint="0 = 不限；按每条约 256 B 估算成内存索引条目数"
                />
              </div>
              <p className="muted">
                目标 v0.8.2 的这几个容量键都没生效（F-015）。本实现让上面两个真正生效；
                segment_cache_max_mb、m3u8_max_entries、segment_max_entries 保留键面但不消费。
              </p>
            </Card>

            <Card title="预载配置" note="仅 Proxy 模式生效">
              <div className="fields">
                <BoolPick label="预载" path="cache.prefetch.enabled" />
                <Num label="拉取并发" path="cache.prefetch.concurrency" unit="个" />
                <Num label="拉取超时" path="cache.prefetch.timeout" unit="秒" />
                <Num
                  label="HLS 预载"
                  path="cache.prefetch.prefetch_limit"
                  unit="片"
                  hint="0 = 抓满清单窗口"
                />
                <Num label="MPD 预载" path="cache.prefetch.mpd_readahead" unit="片" />
                <Num
                  label="跳过末尾"
                  path="cache.prefetch.skip_tail"
                  unit="片"
                  hint="本实现扩展：清单末尾不预取"
                />
              </div>
            </Card>

            <Card
              title="磁盘回收"
              note="1 GB 盘的硬前提"
              actions={
                <button
                  className="ghost"
                  disabled={busy === 'estimate'}
                  onClick={() => void estimate()}
                >
                  {busy === 'estimate' ? '估算中…' : '立即估算'}
                </button>
              }
            >
              <div className="fields">
                <BoolPick label="磁盘回收" path="cache.gc.enabled" />
                <Num label="占用上限" path="cache.gc.max_disk_mb" unit="MB" hint={capHint} />
                <Num label="超龄删除" path="cache.gc.max_age_secs" unit="秒" />
                <Num label="扫描周期" path="cache.gc.interval_secs" unit="秒" />
                <Num label="单轮上限" path="cache.gc.scan_limit" unit="个文件" />
                <Num
                  label="可用占比"
                  path="cache.gc.disk_budget_ratio"
                  step={0.05}
                  hint="自动上限最多占可用空间的比例"
                />
                <Num label="预留" path="cache.gc.reserve_mb" unit="MB" />
                <Num label="自动下限" path="cache.gc.min_mb" unit="MB" />
              </div>
            </Card>

            {recommend ? <RecommendCard recommend={recommend} /> : null}

            <Card title="CDN 兜底" note="外部 CDN 侧不生效时的服务端兜底；默认关闭">
              <div className="fields">
                <BoolPick
                  label="启用 CDN 兜底"
                  path="cdn.enabled"
                  hint="关闭时响应头与状态码与旧版逐字节一致"
                />
                <BoolPick
                  label="被动实测"
                  path="cdn.auto_tune"
                  hint="从真实播放流量推算推荐值"
                />
                <Num
                  label="切片 max-age"
                  path="cdn.segment_max_age_secs"
                  unit="秒，0=自动"
                  hint={ttlHint}
                />
                <Num
                  label="上游失败状态码"
                  path="cdn.empty_status"
                  hint="默认 502：让「200 + 空体」不被 CDN 缓存"
                />
                <Text
                  label="对外基址"
                  path="cdn.public_base"
                  placeholder="https://cdn.example"
                  hint="CDN 不下发 X-Forwarded-Proto 时用；留空则按 Host 推导"
                />
                <Toggle label="命中路径也标 public" path="cdn.shared_cache_hits" />
                <Toggle label="空体加 no-store" path="cdn.dont_cache_empty" />
              </div>
            </Card>

            <Card title="切片缓存" note="本实现扩展：磁盘索引与内存索引上限">
              <div className="fields">
                <Num
                  label="全局条目上限"
                  path="cache.segments.global_max"
                  hint="0 = 不限；与「内存上限」取更小者"
                />
                <Num
                  label="单频道上限"
                  path="cache.segments.per_channel_max"
                  hint="0 = 仅用内置默认 4096"
                />
                <Num label="保留条数" path="cache.segments.retain_count" />
              </div>
            </Card>

            <Card
              title="字幕设置"
              note="仅 DASH 模式频道生效"
              actions={
                <button
                  className="ghost"
                  disabled={busy === 'subclear'}
                  onClick={() => void clearTranslated()}
                >
                  {busy === 'subclear' ? '清理中…' : '清理译文缓存'}
                </button>
              }
            >
              <Toggle
                label="开启（下方参数生效，注入到 VTT）"
                path="subtitle.style.enabled"
                hint="关闭时原样返回字幕"
              />
              <div className="fields">
                <Pick label="字号" path="subtitle.style.font_size" options={FONT_SIZES} />
                <Pick label="颜色" path="subtitle.style.color" options={SUBTITLE_COLORS} />
                <Toggle label="字体加粗" path="subtitle.style.bold" />
                <Range
                  label="位置"
                  path="subtitle.style.position"
                  min={0}
                  max={100}
                  unit="%"
                  hint="越大越靠下"
                />
              </div>
              <Toggle label="字幕机翻" path="subtitle.translate.enabled" />
              <Chips
                label="目标语言"
                path="subtitle.translate.targets"
                primaryPath="subtitle.translate.target"
                options={SUBTITLE_LANGS}
                hint="可多选；第一个选中的作为主目标语言"
              />
              <div className="fields">
                <Toggle label="显示双语" path="subtitle.translate.bilingual" />
                <BoolPick
                  label="显示方式"
                  path="subtitle.translate.translation_first"
                  onLabel="译文在上"
                  offLabel="译文在下"
                />
                <Toggle label="默认选中译轴" path="subtitle.translate.set_default" />
                <Num label="翻译超时" path="subtitle.translate.timeout_ms" unit="ms" />
              </div>
              <div className="fields">
                <Text
                  label="MTranServer"
                  path="subtitle.translate.url"
                  placeholder="http://127.0.0.1:8989"
                  hint="留空则用内置免费机翻"
                />
                <Text label="API Key" path="subtitle.translate.token" hint="没填就留空" />
              </div>
              <div className="row">
                <button className="ghost" disabled={busy === 'mtran'} onClick={() => void testMtran()}>
                  {busy === 'mtran' ? '测试中…' : '测试'}
                </button>
                <span className="muted">测试用的是已保存的配置：改了地址先保存再测。</span>
              </div>
            </Card>

            <Card title="分享（多 VPS）" note="主从互拉分组与订阅；对端用 ?key= 鉴权">
              <div className="fields">
                <Toggle label="启用分享" path="share.enabled" />
                <Pick
                  label="角色"
                  path="share.role"
                  options={[
                    { value: 'master', label: 'master（主机，拉取下线）' },
                    { value: 'follower', label: 'follower（从机，从主机拉取）' },
                  ]}
                  hint="角色会影响「安全管控」的策略"
                />
                <Text label="分享密钥" path="share.key" hint="对端用 ?key= 传入；主从必须一致" />
                <Num
                  label="轮询周期"
                  path="share.poll_secs"
                  unit="秒"
                  hint="实际下限 5 秒"
                />
                <Toggle
                  label="订阅地址带分享 token"
                  path="share.share_token"
                  hint="目标键：本实现仅回显，不改变订阅地址"
                />
              </div>
              <div className="fields">
                <Text
                  label="主机地址（follower 用）"
                  path="share.master_url"
                  placeholder="https://master.example"
                />
                <Lines
                  label="下线地址（master 用）"
                  path="share.follower_urls"
                  rows={2}
                  hint="一行一个；master 逐个拉取"
                />
              </div>
              <Lines
                label="允许拉取的来源"
                path="share.trust_ips"
                rows={2}
                hint="精确 IP 或 a.b.c.0/24；留空表示不限制"
              />
            </Card>

            <Card
              title="DASH 调优"
              note="304 增量刷新、多 CDN 故障切换、断流续传、片段级容错都是自动的，无需配置"
            >
              <p className="muted">
                三组参数默认值已适配绝大多数源，不确定用途就不要改——改错会直接影响起播与卡顿。
              </p>
              <details>
                <summary>播放体验（影响延迟、起播速度、卡顿稳定性）</summary>
                <div className="fields">
                  <Num
                    label="媒体窗口"
                    path="cache.dash.playlist_min_secs"
                    unit="秒"
                    step={0.5}
                    hint="媒体清单保留的时间窗"
                  />
                  <Num
                    label="时移偏移"
                    path="cache.dash.start_time_offset_secs"
                    unit="秒"
                    step={0.5}
                  />
                  <Num
                    label="发布延迟"
                    path="cache.dash.publish_delay_secs"
                    unit="秒"
                    step={0.5}
                    hint=">0 时压后 ceil(延迟/切片时长) 段"
                  />
                  <Num
                    label="保留下限"
                    path="cache.dash.retain_min_secs"
                    unit="秒"
                    step={0.5}
                  />
                  <Num
                    label="额外抓取"
                    path="cache.dash.fetch_extra_segments"
                    unit="段"
                    hint="发布延迟为 0 时压后这么多段"
                  />
                  <Num
                    label="快速起播"
                    path="cache.dash.fast_start_secs"
                    unit="秒"
                    step={0.5}
                    hint="目标侧无可观测差异，保留配置"
                  />
                </div>
              </details>
              <details>
                <summary>网络性能（高级，默认值适用大多数场景）</summary>
                <div className="fields">
                  <Num
                    label="清单刷新下限"
                    path="cache.dash.mpd_refresh_min_secs"
                    unit="秒"
                    step={0.5}
                  />
                  <Num
                    label="清单刷新上限"
                    path="cache.dash.mpd_refresh_max_secs"
                    unit="秒"
                    step={0.5}
                  />
                  <Num label="清单超时" path="cache.dash.mpd_timeout_secs" unit="秒" />
                  <Num label="切片超时" path="cache.dash.segment_timeout_secs" unit="秒" />
                  <Num label="拉取并发" path="cache.dash.fetch_concurrency" unit="个" />
                  <Num label="请求间隔下限" path="cache.dash.min_request_interval_ms" unit="毫秒" />
                  <Num label="重试次数" path="cache.dash.retry_max" unit="次" />
                  <Num label="首次退避" path="cache.dash.retry_initial_ms" unit="毫秒" />
                  <Num
                    label="退避倍数"
                    path="cache.dash.retry_backoff"
                    step={0.5}
                  />
                  <Num label="退避上限" path="cache.dash.retry_max_ms" unit="毫秒" />
                </div>
              </details>
              <details>
                <summary>特殊场景（高级，一般无需调整）</summary>
                <div className="fields">
                  <Num label="看门狗周期" path="cache.dash.watchdog_tick_secs" unit="秒" />
                  <Num label="停滞判定" path="cache.dash.watchdog_stall_secs" unit="秒" />
                  <Num label="重建上限" path="cache.dash.watchdog_rebuild_max" unit="次" />
                  <Num
                    label="重建冷却"
                    path="cache.dash.watchdog_cooldown_secs"
                    unit="秒"
                    hint="连续失败达上限后冷却"
                  />
                  <Num
                    label="重建错开"
                    path="cache.dash.watchdog_rebuild_stagger_ms"
                    unit="毫秒"
                  />
                  <Num label="清单停滞" path="cache.dash.playlist_stall_secs" unit="秒" />
                  <Num label="按需空闲回收" path="cache.dash.ondemand_idle_secs" unit="秒" />
                  <Num
                    label="会话最长存活"
                    path="cache.dash.session_max_lifetime_secs"
                    unit="秒"
                    hint="0 = 不限"
                  />
                  <Num
                    label="尾部连续比"
                    path="cache.dash.continuous_tail_min_ratio"
                    step={0.05}
                  />
                  <Num label="整文件预算" path="cache.dash.whole_file_budget_mb" unit="MB" />
                  <Toggle label="文件名混淆" path="cache.dash.obfuscate_filenames" />
                  <Toggle label="清单正文去重" path="cache.dash.mpd_body_dedup" />
                </div>
                <Pick
                  label="整文件下发"
                  path="cache.dash.whole_file_serve"
                  options={[
                    { value: 'auto', label: '自动（auto）' },
                    { value: 'on', label: '总是' },
                    { value: 'off', label: '从不' },
                  ]}
                  hint="取值以配置为准；目标侧该键存在回读异常（S-CACHE-07）"
                />
              </details>
            </Card>

            <RawCard
              draft={draft}
              onSaved={(next) => {
                setDraft(next);
                setDirty(false);
              }}
            />
          </div>
        </DraftCtx.Provider>
      )}

      <SystemPage />
      <BackupPage />
      {node}
    </>
  );
}

/** 兜底：直接编辑整份 JSON（尚未做成字段级表单的段）。 */
function RawCard({ draft, onSaved }: { draft: Cfg; onSaved: (next: Cfg) => void }) {
  const [raw, setRaw] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const { ok, err } = useToast();

  const save = async () => {
    if (raw === null) return;
    setBusy(true);
    try {
      const parsed = JSON.parse(raw) as Cfg;
      await api('PUT', '/config', parsed);
      onSaved(parsed);
      setRaw(null);
      ok('配置已保存');
    } catch (error) {
      err(error instanceof Error ? error.message : String(error));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="card">
      <details
        onToggle={(event) => {
          if ((event.target as HTMLDetailsElement).open && raw === null) {
            setRaw(JSON.stringify(draft, null, 2));
          }
        }}
      >
        <summary>
          <h2 style={{ display: 'inline' }}>其余字段（JSON）</h2>
        </summary>
        <p className="muted">
          字幕、DASH 调优、分享、在流等段还没有字段级表单，用这里改。保存同样写回整份配置。
        </p>
        <textarea
          rows={16}
          spellCheck={false}
          value={raw ?? ''}
          onChange={(event) => setRaw(event.target.value)}
        />
        <div style={{ marginTop: 12 }}>
          <button disabled={busy} onClick={() => void save()}>
            {busy ? '保存中…' : '保存 JSON'}
          </button>
        </div>
      </details>
    </div>
  );
}
