// Same-origin bearer client. Credentials never enter URLs; session.js alone
// manages persistence of the successfully authenticated read-only viewer token.
export class ApiError extends Error {
  constructor(message, status = 0, retryAfter = 0) { super(message); this.status = status; this.retryAfter = retryAfter; }
}
export class ApiClient {
  constructor(token) { this.token = token; this.abort = new AbortController(); this.snapshots = new Map(); }
  close() { this.abort.abort(); this.token = ''; this.snapshots.clear(); }
  async get(path, params = {}, options = {}) {
    const url = new URL(path, location.origin);
    if (url.origin !== location.origin) throw new ApiError('Use the console hosted by the server itself.');
    for (const [key, value] of Object.entries(params)) if (value != null) url.searchParams.set(key, value);
    if (url.searchParams.has('cursor') && (options.releaseCursor || options.releasePrevious)) throw new ApiError('Snapshot release is only allowed on a new first-page request.');
    const snapshotUrl = new URL(url); snapshotUrl.searchParams.sort();
    const snapshotKey = snapshotUrl.href;
    const releaseCursor = options.releaseCursor || (options.releasePrevious ? this.snapshots.get(snapshotKey) : null);
    this.abort.signal.throwIfAborted();
    const controller = new AbortController();
    const cancel = () => controller.abort();
    this.abort.signal.addEventListener('abort', cancel, { once: true });
    if (this.abort.signal.aborted) cancel();
    const timeout = setTimeout(cancel, 10000);
    try {
      const response = await fetch(url, { headers: { Authorization: `Bearer ${this.token}`, Accept: 'application/json', ...(releaseCursor ? { 'X-Cider-Release-Cursor': releaseCursor } : {}) }, cache: 'no-store', credentials: 'omit', signal: controller.signal });
      const body = await response.json().catch(() => null);
      this.abort.signal.throwIfAborted();
      if (!response.ok) {
        const retry = response.headers.get('Retry-After');
        const seconds = retry && /^\d+$/.test(retry) ? Number(retry) : Math.max(0, (Date.parse(retry || '') - Date.now()) / 1000);
        throw new ApiError(body?.error?.message || `Server returned HTTP ${response.status}`, response.status, Number.isFinite(seconds) ? seconds : 0);
      }
      if (!body?.meta || !Object.prototype.hasOwnProperty.call(body, 'data')) throw new ApiError('The server response does not match the documented API.');
      if (options.releasePrevious) {
        // Keep page one's handle: terminal pages carry next_cursor=null but do
        // not free their frozen server snapshot. Failed refreshes retain it for
        // an idempotent release retry. Closing a client discards all handles.
        if (body.meta.next_cursor) this.snapshots.set(snapshotKey, body.meta.next_cursor);
        else this.snapshots.delete(snapshotKey);
      }
      return body;
    } catch (error) {
      if (error.name === 'AbortError' && !this.abort.signal.aborted) throw new ApiError('The server did not respond within 10 seconds.');
      throw error;
    } finally { clearTimeout(timeout); this.abort.signal.removeEventListener('abort', cancel); }
  }
  waitForRetry(milliseconds) {
    return new Promise((resolve, reject) => {
      const signal = this.abort.signal;
      const cancel = () => { clearTimeout(timer); signal.removeEventListener('abort', cancel); reject(new DOMException('Disconnected', 'AbortError')); };
      const timer = setTimeout(() => { signal.removeEventListener('abort', cancel); resolve(); }, milliseconds);
      signal.addEventListener('abort', cancel, { once: true });
      if (signal.aborted) cancel();
    });
  }
  async all(path, params = {}, options = {}) {
    const data = []; let cursor = null; const seen = new Set();
    do {
      let page;
      // Keep the accepted snapshot pages when its next page exhausts the shared
      // read budget. Restarting the traversal can starve larger inventories.
      for (let attempt = 0; ; attempt++) {
        try { page = await this.get(path, { ...params, limit: 500, cursor }, { releasePrevious: !cursor && options.releasePrevious }); break; }
        catch (error) {
          if (error.status !== 429 || attempt >= 3 || error.retryAfter > 30) throw error;
          await this.waitForRetry(Math.max(1000, (error.retryAfter || 0) * 1000));
        }
      }
      if (!Array.isArray(page.data)) throw new ApiError('Expected a paginated collection.');
      data.push(...page.data); cursor = page.meta.next_cursor;
      if (cursor && seen.has(cursor)) throw new ApiError('The server repeated a pagination cursor.');
      if (cursor) seen.add(cursor);
    } while (cursor);
    return data;
  }
}
