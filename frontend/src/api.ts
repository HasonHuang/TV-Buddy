/** 面板接口客户端。 */

const TOKEN_KEY = 'tv_buddy_token';
const ROLE_KEY = 'tv_buddy_role';

export function getToken(): string {
  return localStorage.getItem(TOKEN_KEY) ?? '';
}

export function setToken(token: string): void {
  localStorage.setItem(TOKEN_KEY, token);
}

/** 登录时记下的角色。只用来决定侧栏显示，权限由服务端判定。 */
export function getRole(): 'admin' | 'user' | null {
  const value = localStorage.getItem(ROLE_KEY);
  return value === 'admin' || value === 'user' ? value : null;
}

export function setRole(role: 'admin' | 'user'): void {
  localStorage.setItem(ROLE_KEY, role);
}

export function clearToken(): void {
  localStorage.removeItem(TOKEN_KEY);
  localStorage.removeItem(ROLE_KEY);
}

/** 带状态码的接口错误。 */
export class ApiError extends Error {
  readonly status: number;
  /** 服务端给的错误码，如 `stale_revision`、`content_exists`。 */
  readonly code: string;

  constructor(status: number, message: string, code = '') {
    super(message);
    this.status = status;
    this.code = code;
  }
}

/** 未登录事件：由 App 监听，用来回到登录卡。 */
export const UNAUTHORIZED_EVENT = 'tv-buddy:unauthorized';

/** 调用 `/panel/api/*`；自动带 Bearer token。401 时清除 token 并广播。 */
export async function api<T>(method: string, path: string, body?: unknown): Promise<T> {
  const headers: Record<string, string> = {};
  const token = getToken();
  if (token) headers.Authorization = `Bearer ${token}`;
  if (body !== undefined) headers['Content-Type'] = 'application/json';

  const response = await fetch(`/panel/api${path}`, {
    method,
    headers,
    body: body === undefined ? undefined : JSON.stringify(body),
  });

  if (response.status === 401) {
    clearToken();
    window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
  }

  if (!response.ok) {
    let message = `${response.status} ${response.statusText}`;
    let code = '';
    try {
      const data = (await response.json()) as { error?: string; code?: string };
      if (data.error) message = data.error;
      if (data.code) code = data.code;
    } catch {
      // 保留默认文案
    }
    throw new ApiError(response.status, message, code);
  }
  return (await response.json()) as T;
}

/** 调用 `/panel/api/*` 并取纯文本正文（如 `text/plain` 的匹配报告）。 */
export async function apiText(method: string, path: string): Promise<string> {
  const headers: Record<string, string> = {};
  const token = getToken();
  if (token) headers.Authorization = `Bearer ${token}`;

  const response = await fetch(`/panel/api${path}`, { method, headers });
  if (response.status === 401) {
    clearToken();
    window.dispatchEvent(new Event(UNAUTHORIZED_EVENT));
  }
  if (!response.ok) {
    let message = `${response.status} ${response.statusText}`;
    const body = await response.text().catch(() => '');
    if (body.trim() !== '') message = body.trim();
    throw new ApiError(response.status, message);
  }
  return response.text();
}

/**
 * 保存整份配置。带上 `revision` 并发令牌。
 *
 * 配置已被别处更新时抛出 `ApiError`，`code = "stale_revision"`。
 * 调用方应重新加载配置，不要覆盖。
 */
export async function putConfig<T extends Record<string, unknown>>(
  config: T,
): Promise<number> {
  const result = await api<{ ok: boolean; revision: number }>('PUT', '/config', config);
  return result.revision;
}

/** 复制文本。剪贴板不可用时回退到临时输入框。 */
export async function copyText(text: string): Promise<boolean> {
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    const input = document.createElement('textarea');
    input.value = text;
    document.body.appendChild(input);
    input.select();
    const ok = document.execCommand('copy');
    document.body.removeChild(input);
    return ok;
  }
}

/** 质量判定原因。与后端结果记录的 `reason` 一致。 */
export type QualityReason =
  | 'pass'
  | 'unreachable'
  | 'speed_low'
  | 'resolution_low'
  | 'resolution_high'
  | 'delay_high'
  | 'probe_error'
  | 'source_removed'
  | 'source_disabled'
  | 'source_changed'
  | 'budget'
  | 'cancelled'
  | 'guard'
  | 'unknown';

/** 结果动作用途。 */
export type QualityAction =
  | 'admitted'
  | 'rejected'
  | 'removed'
  | 'kept'
  | 'hidden'
  | 'source_removed'
  | 'held'
  | 'removal_paused'
  | 'skipped';

/** 单频道质量结果。字段来自 `GET /quality/results`。 */
export type QualityResult = {
  key: string;
  scope: 'sync' | 'manual' | string;
  source_id: string;
  source_key: string;
  gid: string;
  cid: string;
  name: string;
  target_group: string;
  /** 地址哈希，不包含完整地址或查询令牌。 */
  url_hash: string;
  action: QualityAction | string;
  reason: QualityReason;
  ok: boolean;
  delay_ms: number | null;
  speed_mib: number | null;
  resolution: string;
  http_status: number | null;
  checked_at: number;
  next_check_at: number;
  first_fail_at: number | null;
  rule_hash: string;
  /** 分组页徽章兼容字段；结果接口不返回时按最新结果推导。 */
  hidden?: boolean;
  stale?: boolean;
};

/** 一次校验任务的进度。 */
export type QualityRun = {
  run_id: string;
  trigger: string;
  phase: string;
  started_at: number;
  total: number;
  tested: number;
  valid: number;
  admitted: number;
  removed: number;
  rejected: number;
  skipped: number;
  hidden: number;
  bytes: number;
  cancelled: boolean;
  finished: boolean;
  paused_by_guard: boolean;
  measure_only: boolean;
  error: string | null;
};

export type QualityStatus = {
  enabled: boolean;
  quality_enabled: boolean;
  running: boolean;
  run: QualityRun | null;
  schedule: {
    mode: 'interval' | 'cron' | string;
    interval_hours: number;
    cron: string;
    next_run_at: number;
  };
  counts: {
    admitted: number;
    removed_7d: number;
    probe_errors_7d: number;
    sources: number;
  };
  source_errors: Array<{ id: string; name: string; error: string }>;
  guard: {
    enabled: boolean;
    max_remove_ratio: number;
    min_remove_count: number;
    tripped: boolean;
  };
  ffprobe: {
    installed: boolean;
    enabled: boolean;
    ready: boolean;
    note: string;
  };
};

/** 同步源的门控状态。 */
export type QualitySource = {
  id: string;
  name: string;
  enabled: boolean;
  channels: number;
  gate_enabled: boolean;
  phase: string;
  error: string | null;
  tested: number;
  admitted: number;
  rejected: number;
  removed: number;
  paused: boolean;
  pause_reason: string;
  truncated: boolean;
  cursor: string;
  remaining: number;
  last_at: number;
};

/** 某个频道或全局的有效规则。 */
export type QualityEffective = {
  rules: {
    enabled: boolean;
    min_speed_mib: number;
    fallback_speed_mib: number;
    max_delay_ms: number;
    min_resolution_px: number | null;
    max_resolution_px: number | null;
    host_cache: boolean;
  };
  source_layer: 'channel' | 'group' | 'source' | 'global' | string;
};

export type QualityScope = 'all' | 'source' | 'group' | 'channel';

/** 读取质量校验状态。 */
export function fetchQualityStatus(): Promise<QualityStatus> {
  return api<QualityStatus>('GET', '/quality/status');
}

/** 读取质量结果。可按来源、分组、频道、动作、原因与关键字过滤。 */
export function fetchQualityResults(params: {
  source_id?: string;
  gid?: string;
  cid?: string;
  action?: string;
  reason?: string;
  q?: string;
  limit?: number;
  offset?: number;
} = {}): Promise<{ total: number; items: QualityResult[] }> {
  const search = new URLSearchParams();
  if (params.source_id) search.set('source_id', params.source_id);
  if (params.gid) search.set('gid', params.gid);
  if (params.cid) search.set('cid', params.cid);
  if (params.action) search.set('action', params.action);
  if (params.reason) search.set('reason', params.reason);
  if (params.q) search.set('q', params.q);
  if (params.limit !== undefined) search.set('limit', String(params.limit));
  if (params.offset !== undefined) search.set('offset', String(params.offset));
  const query = search.toString();
  return api<{ total: number; items: QualityResult[] }>(
    'GET',
    `/quality/results${query ? `?${query}` : ''}`,
  );
}

/** 读取同步源门控统计。 */
export function fetchQualitySources(): Promise<{ items: QualitySource[] }> {
  return api<{ items: QualitySource[] }>('GET', '/quality/sources');
}

/** 读取有效规则与来源层级。 */
export function fetchQualityEffective(params: {
  source_id?: string;
  gid?: string;
  cid?: string;
} = {}): Promise<QualityEffective> {
  const search = new URLSearchParams();
  if (params.source_id) search.set('source_id', params.source_id);
  if (params.gid) search.set('gid', params.gid);
  if (params.cid) search.set('cid', params.cid);
  const query = search.toString();
  return api<QualityEffective>('GET', `/quality/effective${query ? `?${query}` : ''}`);
}

/** 发起一次质量校验。`id` 在非 `all` 范围必填；频道用 `gid/cid`。 */
export function runQuality(
  scope: QualityScope,
  id?: string,
  mode: 'quick' | 'full' = 'quick',
): Promise<{ ok: boolean; run_id: string; queued: number; measure_only: boolean }> {
  return api<{ ok: boolean; run_id: string; queued: number; measure_only: boolean }>(
    'POST',
    '/quality/run',
    {
      scope,
      ...(id ? { id } : {}),
      run_mode: mode,
    },
  );
}

/** 取消当前质量任务。 */
export function cancelQuality(): Promise<{ ok: boolean; running: boolean }> {
  return api<{ ok: boolean; running: boolean }>('POST', '/quality/cancel');
}

/** 预览 cron 或按小时周期的未来 3 次运行时间。 */
export function previewQualitySchedule(input: {
  mode: 'interval' | 'cron';
  interval_hours?: number;
  cron?: string;
}): Promise<{ times: number[] }> {
  return api<{ times: number[] }>('POST', '/quality/schedule/preview', input);
}

/** 订阅源配置。字段较多，质量页只读写需要的一小部分。 */
export type PlaylistSourceConfig = {
  id: string;
  name?: string;
  mode?: string;
  enabled?: boolean;
  quality?: Record<string, unknown> | null;
  [key: string]: unknown;
};

/** 读取订阅源配置。 */
export function fetchPlaylistSources(): Promise<{ sources: PlaylistSourceConfig[] }> {
  return api<{ sources: PlaylistSourceConfig[] }>('GET', '/playlist');
}

/** 保存一个订阅源。 */
export function updatePlaylistSource(
  id: string,
  source: PlaylistSourceConfig,
): Promise<{ ok: boolean; source: PlaylistSourceConfig; detached: number }> {
  return api<{ ok: boolean; source: PlaylistSourceConfig; detached: number }>(
    'PUT',
    `/playlist/sources/${encodeURIComponent(id)}`,
    source,
  );
}

/** 触发一个托管源同步。 */
export function syncPlaylistSource(id: string): Promise<{ ok: boolean; queued?: number }> {
  return api<{ ok: boolean; queued?: number }>('POST', '/subs/sync', { id });
}

/** 停用源。`keepChannels=false` 时同时移出频道。 */
export function disableQualitySource(
  id: string,
  keepChannels: boolean,
): Promise<{ ok: boolean }> {
  return api<{ ok: boolean }>('POST', `/playlist/sources/${encodeURIComponent(id)}/disable`, {
    keep_channels: keepChannels,
  });
}

/** 启用源；同步源会排入同步。 */
export function enableQualitySource(id: string): Promise<{ ok: boolean; queued: boolean }> {
  return api<{ ok: boolean; queued: boolean }>(
    'POST',
    `/playlist/sources/${encodeURIComponent(id)}/enable`,
  );
}

/** 删除源。`keepContent=true` 时频道保留为本地频道。 */
export function deleteQualitySource(
  id: string,
  keepContent: boolean,
): Promise<{ ok: boolean; channels: number; to_manual: boolean }> {
  return api<{ ok: boolean; channels: number; to_manual: boolean }>(
    'DELETE',
    `/playlist/sources/${encodeURIComponent(id)}?keep_content=${keepContent ? 1 : 0}`,
  );
}
