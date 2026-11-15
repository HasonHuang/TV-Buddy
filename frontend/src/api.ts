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

/** 质量判定原因。与后端 `Reason::as_str` 一致。 */
export type QualityReason =
  | 'pass'
  | 'retained'
  | 'unreachable'
  | 'speed_low'
  | 'resolution_low'
  | 'resolution_high'
  | 'delay_high'
  | 'supply'
  | 'probe_error'
  | 'unknown';

/** 单频道质量结果。字段来自 `GET /quality/results`。 */
export type QualityResult = {
  gid: string;
  cid: string;
  name: string;
  source_id: string;
  checked_at: number;
  next_check_at: number;
  last_ok_at: number | null;
  first_fail_at: number | null;
  ok: boolean;
  reachable: boolean;
  reason: QualityReason;
  delay_ms: number;
  speed_mib: number;
  width: number;
  height: number;
  resolution: string;
  consecutive_fails: number;
  flags: number;
  hidden: boolean;
  stale: boolean;
};

/** 一次校验任务的进度。 */
export type QualityRun = {
  run_id: string;
  started_at: number;
  total: number;
  tested: number;
  valid: number;
  hidden: number;
  bytes: number;
  cancelled: boolean;
  finished: boolean;
  error: string | null;
};

export type QualityStatus = {
  enabled: boolean;
  probe_mode: string;
  interval_secs: number;
  running: boolean;
  run: QualityRun | null;
  rules: {
    action: string;
    retain: boolean;
    supply: boolean;
    min_speed_mib: number;
    max_delay_ms: number;
    min_resolution: string;
    max_resolution: string;
    stale_keep_secs: number;
  };
  ffprobe: { installed: boolean; required: boolean };
  entries: number;
};

export type QualityScope = 'all' | 'source' | 'group' | 'channel';

/** 读取质量校验状态。 */
export function fetchQualityStatus(): Promise<QualityStatus> {
  return api<QualityStatus>('GET', '/quality/status');
}

/** 读取质量结果。可按分组、频道过滤。 */
export function fetchQualityResults(params: {
  gid?: string;
  cid?: string;
  limit?: number;
} = {}): Promise<{ total: number; items: QualityResult[] }> {
  const search = new URLSearchParams();
  if (params.gid) search.set('gid', params.gid);
  if (params.cid) search.set('cid', params.cid);
  if (params.limit !== undefined) search.set('limit', String(params.limit));
  const query = search.toString();
  return api<{ total: number; items: QualityResult[] }>(
    'GET',
    `/quality/results${query ? `?${query}` : ''}`,
  );
}

/** 发起一次质量校验。`id` 在非 `all` 范围必填；频道用 `gid/cid`。 */
export function runQuality(
  scope: QualityScope,
  id?: string,
  mode: 'quick' | 'full' = 'quick',
  force = false,
): Promise<{ ok: boolean; run_id: string; queued: boolean }> {
  return api<{ ok: boolean; run_id: string; queued: boolean }>('POST', '/quality/run', {
    scope,
    ...(id ? { id } : {}),
    run_mode: mode,
    force,
  });
}
