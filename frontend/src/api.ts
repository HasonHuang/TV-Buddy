/** 面板接口客户端。 */

const TOKEN_KEY = 'tv_buddy_token';

export function getToken(): string {
  return localStorage.getItem(TOKEN_KEY) ?? '';
}

export function setToken(token: string): void {
  localStorage.setItem(TOKEN_KEY, token);
}

export function clearToken(): void {
  localStorage.removeItem(TOKEN_KEY);
}

/** 带状态码的接口错误。 */
export class ApiError extends Error {
  readonly status: number;

  constructor(status: number, message: string) {
    super(message);
    this.status = status;
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
    try {
      const data = (await response.json()) as { error?: string };
      if (data.error) message = data.error;
    } catch {
      // 保留默认文案
    }
    throw new ApiError(response.status, message);
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
