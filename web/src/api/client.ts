// The only code in this application that talks to the network
// (docs/domain/operator-auth-http-web-grill.md §0.32, issue #60's own
// acceptance criteria). A route handler that fetched directly would be free
// to forget the `X-Salt-Request` header or the credentials mode, and either
// mistake looks like a working feature until the day it is not.

const MUTATING_METHODS = new Set(['POST', 'PUT', 'PATCH', 'DELETE']);

/** Salt's own envelope (§0.23): `{ "error": { code, message, details } }`. */
export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details: unknown;

  constructor(status: number, code: string, message: string, details: unknown) {
    super(message);
    this.name = 'ApiError';
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

// A 401 means "you are signed out", from any request, at any time (§0.33).
// This lives as a plain event target rather than a React hook because the
// wrapper below has no component tree to call `useNavigate` from — the
// listener that actually redirects is registered by `UnauthorizedRedirect`.
const unauthorized = new EventTarget();

export function onUnauthorized(listener: () => void): () => void {
  unauthorized.addEventListener('unauthorized', listener);
  return () => unauthorized.removeEventListener('unauthorized', listener);
}

export async function apiFetch<T>(path: string, init: RequestInit = {}): Promise<T> {
  const method = (init.method ?? 'GET').toUpperCase();
  const headers = new Headers(init.headers);

  if (init.body !== undefined && !headers.has('Content-Type')) {
    headers.set('Content-Type', 'application/json');
  }

  // The client half of §0.14's CSRF defence. Every mutating call goes
  // through this one function, so no call site can forget it.
  if (MUTATING_METHODS.has(method)) {
    headers.set('X-Salt-Request', '1');
  }

  const response = await fetch(path, {
    ...init,
    method,
    headers,
    // Same-origin only: development proxies `/api` onto one origin (§0.15)
    // and production is one origin behind Caddy (§0.20), so there is never
    // a cross-origin request that would need a wider mode.
    credentials: 'same-origin',
  });

  if (response.status === 401) {
    unauthorized.dispatchEvent(new Event('unauthorized'));
  }

  if (!response.ok) {
    throw await readApiError(response);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return (await response.json()) as T;
}

async function readApiError(response: Response): Promise<ApiError> {
  const body: unknown = await response.json().catch(() => null);
  const envelope = body as { error?: { code?: string; message?: string; details?: unknown } } | null;
  const error = envelope?.error;

  return new ApiError(
    response.status,
    error?.code ?? 'unknown_error',
    error?.message ?? 'an unexpected error occurred',
    error?.details ?? null,
  );
}
