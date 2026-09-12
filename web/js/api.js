// Same-origin bearer client. Credentials never enter URLs or browser storage.
export class ApiError extends Error {
  constructor(message, status = 0, retryAfter = 0) { super(message); this.status = status; this.retryAfter = retryAfter; }
}
export class ApiClient {
  constructor(token) { this.token = token; this.abort = new AbortController(); }
  close() { this.abort.abort(); this.token = ''; }
  async get(path, params = {}) {
    const url = new URL(path, location.origin);
    if (url.origin !== location.origin) throw new ApiError('Use the console hosted by the server itself.');
    for (const [key, value] of Object.entries(params)) if (value != null) url.searchParams.set(key, value);
    const controller = new AbortController();
    const cancel = () => controller.abort();
    this.abort.signal.addEventListener('abort', cancel, { once: true });
    if (this.abort.signal.aborted) cancel();
    const timeout = setTimeout(cancel, 10000);
    try {
      const response = await fetch(url, { headers: { Authorization: `Bearer ${this.token}`, Accept: 'application/json' }, cache: 'no-store', credentials: 'omit', signal: controller.signal });
      const body = await response.json().catch(() => null);
      if (!response.ok) {
        const retry = response.headers.get('Retry-After');
        const seconds = retry && /^\d+$/.test(retry) ? Number(retry) : Math.max(0, (Date.parse(retry || '') - Date.now()) / 1000);
        throw new ApiError(body?.error?.message || `Server returned HTTP ${response.status}`, response.status, Number.isFinite(seconds) ? seconds : 0);
      }
      if (!body?.meta || !Object.prototype.hasOwnProperty.call(body, 'data')) throw new ApiError('The server response does not match the documented API.');
      return body;
    } catch (error) {
      if (error.name === 'AbortError' && !this.abort.signal.aborted) throw new ApiError('The server did not respond within 10 seconds.');
      throw error;
    } finally { clearTimeout(timeout); this.abort.signal.removeEventListener('abort', cancel); }
  }
  async all(path, params = {}) {
    const data = []; let cursor = null; const seen = new Set();
    do {
      const page = await this.get(path, { ...params, limit: 500, cursor });
      if (!Array.isArray(page.data)) throw new ApiError('Expected a paginated collection.');
      data.push(...page.data); cursor = page.meta.next_cursor;
      if (cursor && seen.has(cursor)) throw new ApiError('The server repeated a pagination cursor.');
      if (cursor) seen.add(cursor);
    } while (cursor);
    return data;
  }
}
