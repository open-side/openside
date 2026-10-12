// Playwright-compatible prelude. Runs inside QuickJS; every page operation is a call to the Rust
// host (`__openside_op`), which talks CDP. The only code sent to the page is page script the
// caller asked for (`evaluate`, `addInitScript`, `page.cdp`; ADR-022), built in its section below.
(() => {
  'use strict';
  const op = globalThis.__openside_op;
  const log = globalThis.__openside_log;
  const state = { url: String(globalThis.__openside_url || '') };
  delete globalThis.__openside_op;
  delete globalThis.__openside_log;
  delete globalThis.__openside_url;

  // A retry can fix a wait that ran out and a server that could not be reached; nothing else.
  const TRANSIENT = new Set(['TimeoutError', 'NetworkError']);
  function named(name, message) {
    const error = new Error(message);
    error.name = name;
    error.transient = TRANSIENT.has(name);
    return error;
  }
  function unsupported(api, why) {
    throw named('NotSupported', `${api} is not supported by the openside REPL${why ? ` (${why})` : ''}`);
  }
  async function call(name, args) {
    const reply = JSON.parse(await op(name, JSON.stringify(args === undefined ? {} : args)));
    if (typeof reply.url === 'string' && reply.url) state.url = reply.url;
    if (reply.error !== undefined) throw named(reply.name || 'Error', reply.error);
    return reply.value;
  }
  const sleep = (ms) => call('sleep', { ms });

  // Known Playwright APIs that the REPL does not offer get a reason; anything else unknown
  // still throws NotSupported instead of silently returning undefined.
  const REASONS = {
    addScriptTag: 'use page.evaluate or page.addInitScript', addStyleTag: 'use page.evaluate or page.addInitScript',
    exposeFunction: 'page code cannot call back into the REPL', exposeBinding: 'page code cannot call back into the REPL',
    route: 'no network interception',
    unroute: 'no network interception', request: 'no raw network access', context: 'one tab per REPL call',
    screenshot: 'use the browser_screenshot tool', pdf: 'not available', setContent: 'navigate with page.goto',
    dispatchEvent: 'use locator.evaluate',
    elementHandle: 'use locators, or locator.evaluateHandle for a page object',
    elementHandles: 'use locators, or locator.evaluateHandle for a page object', $: 'use locators, or locator.evaluateHandle for a page object',
    $$: 'use locators, or locator.evaluateHandle for a page object',
  };
  function strict(target, prefix) {
    return new Proxy(target, {
      get(object, property, receiver) {
        if (typeof property === 'symbol' || property in object) return Reflect.get(object, property, receiver);
        if (property === 'then' || property === 'toJSON' || property === 'constructor') return undefined;
        const api = `${prefix}.${String(property)}`;
        // Namespaces such as `page.mouse` must fail on member access as well as on call.
        return new Proxy(() => {}, {
          apply: () => unsupported(api, REASONS[property]),
          get: (_, member) => (member === 'then' || typeof member === 'symbol' ? undefined : unsupported(api, REASONS[property])),
        });
      },
    });
  }

  function text(value, exact) {
    if (value instanceof RegExp) return { source: value.source, flags: value.flags };
    return { text: String(value), exact: !!exact };
  }
  function quote(value) {
    return value instanceof RegExp ? String(value) : JSON.stringify(String(value));
  }

  // Playwright selector strings: `css >> css`, `text=`, `role=`, `aria-ref=`, `nth=`, `visible=`,
  // `data-testid=`, and trailing `:has-text()` / `:text()` / `:visible` on CSS.
  function splitChain(selector) {
    const parts = [];
    let depth = 0, quoteChar = '', current = '';
    for (let i = 0; i < selector.length; i++) {
      const c = selector[i];
      if (quoteChar) { if (c === '\\') { current += c + (selector[++i] || ''); continue; } if (c === quoteChar) quoteChar = ''; current += c; continue; }
      if (c === '"' || c === "'") { quoteChar = c; current += c; continue; }
      if (c === '[' || c === '(') depth++;
      if (c === ']' || c === ')') depth--;
      if (depth === 0 && selector.startsWith('>>', i)) { parts.push(current.trim()); current = ''; i++; continue; }
      current += c;
    }
    parts.push(current.trim());
    return parts.filter((part) => part.length);
  }
  function unquote(value) {
    const trimmed = value.trim();
    if (/^".*"[is]?$/s.test(trimmed) || /^'.*'[is]?$/s.test(trimmed)) {
      const flag = /[is]$/.test(trimmed) && !/["']$/.test(trimmed) ? trimmed.slice(-1) : '';
      const body = flag ? trimmed.slice(1, -2) : trimmed.slice(1, -1);
      return { value: body, quoted: true, flag };
    }
    return { value: trimmed, quoted: false, flag: '' };
  }
  function textEngine(body) {
    const regex = /^\/(.*)\/([a-z]*)$/s.exec(body.trim());
    if (regex) return { source: regex[1], flags: regex[2] };
    const parsed = unquote(body);
    return { text: parsed.value, exact: parsed.quoted && parsed.flag !== 'i' };
  }
  function parseSelector(selector) {
    const steps = [];
    for (const part of splitChain(String(selector))) {
      const eq = /^([a-zA-Z-]+)=(.*)$/s.exec(part);
      const engine = eq ? eq[1] : '';
      const body = eq ? eq[2] : part;
      if (engine === 'aria-ref') steps.push({ kind: 'ref', reference: body.trim() });
      else if (engine === 'text') steps.push({ kind: 'text', matcher: textEngine(body) });
      else if (engine === 'nth') steps.push({ kind: 'nth', index: Number(body) });
      else if (engine === 'visible') steps.push({ kind: 'visible', visible: body.trim() !== 'false' });
      else if (engine === 'data-testid' || engine === 'id' || engine === 'data-test-id' || engine === 'data-test') {
        const attr = engine === 'id' ? 'id' : engine;
        steps.push({ kind: 'attr', name: attr, matcher: { text: unquote(body).value, exact: true } });
      } else if (engine === 'role') {
        const match = /^([a-z]+)\s*(.*)$/s.exec(body.trim());
        if (!match) throw named('Error', `invalid role selector ${part}`);
        const step = { kind: 'role', role: match[1], states: {} };
        for (const attr of match[2].matchAll(/\[\s*([a-z-]+)\s*(?:=\s*("(?:[^"\\]|\\.)*"[is]?|'(?:[^'\\]|\\.)*'[is]?|\/(?:[^/\\]|\\.)*\/[a-z]*|[^\]]+))?\s*\]/g)) {
          const key = attr[1];
          const raw = attr[2] === undefined ? 'true' : attr[2];
          if (key === 'name') {
            const regex = /^\/(.*)\/([a-z]*)$/s.exec(raw);
            if (regex) step.name = { source: regex[1], flags: regex[2] };
            else { const parsed = unquote(raw); step.name = { text: parsed.value, exact: parsed.quoted && parsed.flag !== 'i' }; }
          } else if (key === 'level') step.states.level = Number(raw);
          else step.states[key] = raw === 'true' ? true : raw === 'false' ? false : unquote(raw).value;
        }
        steps.push(step);
      } else if (engine === 'xpath' || part.startsWith('/') || part.startsWith('..')) {
        const selector = (engine === 'xpath' ? body : part).trim();
        if (!selector) throw named('Error', `empty XPath selector in ${JSON.stringify(part)}`);
        steps.push({ kind: 'xpath', selector });
      } else if (engine === 'internal' || (eq && engine !== 'css' && !/[\s.#\[:>~+]/.test(engine))) {
        unsupported(`selector engine ${engine}=`);
      } else {
        let css = engine === 'css' ? body : part;
        const filters = [];
        for (;;) {
          const tail = /:(has-text|text|text-is)\(\s*("(?:[^"\\]|\\.)*"|'(?:[^'\\]|\\.)*')\s*\)$|:visible$/s.exec(css);
          if (!tail) break;
          css = css.slice(0, tail.index);
          if (tail[0] === ':visible') filters.unshift({ kind: 'visible', visible: true });
          else {
            const value = unquote(tail[2]).value;
            filters.unshift(tail[1] === 'has-text'
              ? { kind: 'has_text', matcher: { text: value, exact: false }, negate: false }
              : { kind: 'has_text', matcher: { text: value, exact: tail[1] === 'text-is' }, negate: false });
          }
        }
        if (/:has-text\(|:text\(|:is\(|>>|:nth-match\(|:right-of\(|:left-of\(|:near\(|:above\(|:below\(/.test(css)) {
          unsupported(`Playwright pseudo-class in ${JSON.stringify(part)}`, 'only trailing :has-text(), :text() and :visible are supported');
        }
        steps.push({ kind: 'css', selector: css.trim() || '*' }, ...filters);
      }
    }
    return steps;
  }

  class Locator {
    constructor(steps, description) {
      this._steps = steps;
      this._description = description;
    }
    _with(steps, suffix) {
      return wrapLocator(new Locator(this._steps.concat(steps), this._description + suffix));
    }
    _args(extra) {
      return Object.assign({ steps: this._steps, description: this._description }, extra);
    }
    toString() { return this._description; }
    toJSON() { return this._description; }
    locator(selector, options) {
      let result = this._with(parseSelector(selector), `.locator(${quote(selector)})`);
      if (options) result = result.filter(options);
      return result;
    }
    getByRole(role, options = {}) {
      const step = { kind: 'role', role: String(role), states: {} };
      if (options.name !== undefined) step.name = text(options.name, options.exact);
      for (const key of ['checked', 'disabled', 'expanded', 'pressed', 'selected', 'level']) {
        if (options[key] !== undefined) step.states[key] = options[key];
      }
      if (options.includeHidden) unsupported('getByRole({ includeHidden })');
      return this._with([step], `.getByRole(${quote(role)}${options.name !== undefined ? `, { name: ${quote(options.name)} }` : ''})`);
    }
    getByText(value, options = {}) { return this._with([{ kind: 'text', matcher: text(value, options.exact) }], `.getByText(${quote(value)})`); }
    getByLabel(value, options = {}) { return this._with([{ kind: 'label', matcher: text(value, options.exact) }], `.getByLabel(${quote(value)})`); }
    getByPlaceholder(value, options = {}) { return this._with([{ kind: 'attr', name: 'placeholder', matcher: text(value, options.exact) }], `.getByPlaceholder(${quote(value)})`); }
    getByAltText(value, options = {}) { return this._with([{ kind: 'attr', name: 'alt', matcher: text(value, options.exact) }], `.getByAltText(${quote(value)})`); }
    getByTitle(value, options = {}) { return this._with([{ kind: 'attr', name: 'title', matcher: text(value, options.exact) }], `.getByTitle(${quote(value)})`); }
    getByTestId(value) { return this._with([{ kind: 'attr', name: 'data-testid', matcher: value instanceof RegExp ? text(value) : { text: String(value), exact: true } }], `.getByTestId(${quote(value)})`); }
    nth(index) { return this._with([{ kind: 'nth', index: Number(index) }], `.nth(${Number(index)})`); }
    first() { return this._with([{ kind: 'nth', index: 0 }], '.first()'); }
    last() { return this._with([{ kind: 'nth', index: -1 }], '.last()'); }
    filter(options = {}) {
      const steps = [];
      if (options.hasText !== undefined) steps.push({ kind: 'has_text', matcher: text(options.hasText), negate: false });
      if (options.hasNotText !== undefined) steps.push({ kind: 'has_text', matcher: text(options.hasNotText), negate: true });
      if (options.has !== undefined) steps.push({ kind: 'has', steps: options.has._steps, negate: false });
      if (options.hasNot !== undefined) steps.push({ kind: 'has', steps: options.hasNot._steps, negate: true });
      if (options.visible !== undefined) steps.push({ kind: 'visible', visible: !!options.visible });
      return this._with(steps, '.filter(...)');
    }
    contentFrame() { return new FrameLocator(this, `${this._description}.contentFrame()`); }
    frameLocator(selector) { return new FrameLocator(this.locator(selector), `${this._description}.frameLocator(${quote(selector)})`); }
    async count() { return call('count', this._args()); }
    async all() {
      const count = await this.count();
      return Array.from({ length: count }, (_, index) => this.nth(index));
    }
    async _act(action, value, options = {}) {
      return call('act', this._args({ action, value, options, timeout: options.timeout }));
    }
    click(options) { return this._act('click', null, options); }
    dblclick(options) { return this._act('dblclick', null, options); }
    hover(options) { return this._act('hover', null, options); }
    focus(options) { return this._act('focus', null, options); }
    fill(value, options) { return this._act('fill', String(value), options); }
    clear(options) { return this._act('fill', '', options); }
    type(value, options) { return this._act('type', String(value), options); }
    pressSequentially(value, options) { return this._act('type', String(value), options); }
    press(key, options) { return this._act('press', String(key), options); }
    check(options) { return this._act('check', true, options); }
    uncheck(options) { return this._act('check', false, options); }
    setChecked(checked, options) { return this._act('check', !!checked, options); }
    selectOption(values, options) { return this._act('select', values, options); }
    dragTo(target, options = {}) {
      if (!(target instanceof Locator)) throw named('Error', 'locator.dragTo: target must be a locator');
      return this._act('drag', { steps: target._steps, description: target._description }, options);
    }
    scrollIntoViewIfNeeded(options) { return this._act('scroll', null, options); }
    async _query(what, extra = {}, options = {}) {
      return call('query', this._args(Object.assign({ what, timeout: options.timeout }, extra)));
    }
    textContent(options) { return this._query('textContent', {}, options); }
    innerText(options) { return this._query('innerText', {}, options); }
    innerHTML() { unsupported('locator.innerHTML', 'use textContent or page.content()'); }
    getAttribute(name, options) { return this._query('getAttribute', { name: String(name) }, options); }
    inputValue(options) { return this._query('inputValue', {}, options); }
    isVisible() { return this._query('isVisible'); }
    isHidden() { return this._query('isHidden'); }
    isChecked(options) { return this._query('isChecked', {}, options); }
    isEnabled(options) { return this._query('isEnabled', {}, options); }
    isDisabled(options) { return this._query('isDisabled', {}, options); }
    isEditable(options) { return this._query('isEditable', {}, options); }
    boundingBox(options) { return this._query('boundingBox', {}, options); }
    _focused(options) { return this._query('isFocused', {}, options); }
    allTextContents() { return call('texts', this._args({ inner: false })); }
    allInnerTexts() { return call('texts', this._args({ inner: true })); }
    waitFor(options = {}) { return call('wait_for', this._args({ state: options.state || 'visible', timeout: options.timeout })); }
    setInputFiles(files, options = {}) { return call('set_input_files', this._args({ files: fileList(files), timeout: options.timeout })); }
  }
  function wrapLocator(locator) { return strict(locator, 'locator'); }
  const root = new Locator([], 'page');
  const rootLocator = (selector, options) => root.locator(selector, options);
  const delegate = (method) => (selector, ...args) => rootLocator(selector)[method](...args);

  // Locator methods that start a query inside a document (a frame's, here).
  const QUERIES = ['locator', 'getByRole', 'getByText', 'getByLabel', 'getByPlaceholder', 'getByAltText', 'getByTitle', 'getByTestId'];
  // `owner` locates the <iframe> elements; locators made from here search their documents.
  class FrameLocator {
    constructor(owner, description) {
      this._owner = owner;
      this._description = description;
      this._root = new Locator(owner._steps.concat([{ kind: 'frame' }]), description);
      return strict(this, 'frameLocator');
    }
    toString() { return this._description; }
    toJSON() { return this._description; }
    owner() { return this._owner; }
    frameLocator(selector) { return new FrameLocator(this._root.locator(selector), `${this._description}.frameLocator(${quote(selector)})`); }
    first() { return new FrameLocator(this._owner.first(), `${this._description}.first()`); }
    last() { return new FrameLocator(this._owner.last(), `${this._description}.last()`); }
    nth(index) { return new FrameLocator(this._owner.nth(index), `${this._description}.nth(${Number(index)})`); }
  }
  // A frame is the steps that select its document: `[]` for the main frame, a frame id for
  // frames listed by page.frames(), a name/URL match for page.frame().
  class Frame {
    constructor(steps, description, info, listing) {
      this._steps = steps;
      this._description = description;
      this._info = info || null;
      this._listing = listing || null;
      this._root = new Locator(steps, description);
      return strict(this, 'frame');
    }
    toString() { return this._description; }
    toJSON() { return this._description; }
    name() { return this._info ? this._info.name : ''; }
    url() { return this._steps.length === 0 ? state.url : this._info ? this._info.url : ''; }
    page() { return page; }
    isDetached() { return false; }
    parentFrame() {
      if (!this._info || !this._info.parentId || !this._listing) return null;
      return this._listing.find((frame) => frame._info.id === this._info.parentId) || null;
    }
    childFrames() { return this._listing && this._info ? this._listing.filter((frame) => frame._info.parentId === this._info.id) : []; }
    frameLocator(selector) { return new FrameLocator(this._root.locator(selector), `${this._description}.frameLocator(${quote(selector)})`); }
    title() { return call('title', { frame: this._steps }); }
    content() { return call('content', { frame: this._steps }); }
  }
  for (const name of QUERIES) {
    FrameLocator.prototype[name] = function (...args) { return this._root[name](...args); };
    Frame.prototype[name] = function (...args) { return this._root[name](...args); };
  }
  for (const name of ['click', 'dblclick', 'fill', 'type', 'press', 'check', 'uncheck', 'hover', 'focus', 'selectOption',
    'textContent', 'innerText', 'getAttribute', 'inputValue', 'isVisible', 'isHidden', 'isChecked', 'isEnabled',
    'isDisabled', 'isEditable', 'setInputFiles']) {
    Frame.prototype[name] = function (selector, ...args) { return this._root.locator(selector)[name](...args); };
  }
  Frame.prototype.waitForSelector = async function (selector, options = {}) { await this._root.locator(selector).waitFor(options); return null; };
  const mainFrame = new Frame([], 'page.mainFrame()');
  function frameUrlMatcher(url) {
    if (url instanceof RegExp) return text(url);
    if (typeof url === 'function') unsupported('page.frame({ url: predicate })', 'pass a string, glob or RegExp');
    const pattern = String(url);
    return /[*?{]/.test(pattern) ? { source: globSource(pattern), flags: '' } : { text: pattern, exact: true };
  }
  function pageFrame(selector) {
    const options = typeof selector === 'string' ? { name: selector } : selector || {};
    const step = { kind: 'frame_match' };
    if (options.name !== undefined) step.name = String(options.name);
    if (options.url !== undefined) step.url = frameUrlMatcher(options.url);
    const info = options.name !== undefined ? { name: String(options.name), url: '' } : null;
    return new Frame([step], `page.frame(${typeof selector === 'string' ? quote(selector) : JSON.stringify({ name: options.name, url: options.url === undefined ? undefined : String(options.url) })})`, info);
  }
  async function pageFrames() {
    const listing = [];
    for (const [position, info] of (await call('frames')).entries()) {
      listing.push(position === 0 ? new Frame([], 'page.mainFrame()', info, listing)
        : new Frame([{ kind: 'frame_id', id: info.id }], `page.frames()[${position}]`, info, listing));
    }
    return listing;
  }

  // The sandbox has no URL class; predicates get this read-only view of the URL parts instead.
  function parseUrl(href) {
    const match = /^([a-zA-Z][a-zA-Z0-9+.-]*:)(?:\/\/([^/?#]*))?([^?#]*)(\?[^#]*)?(#.*)?$/.exec(href);
    if (!match) return { href, toString: () => href };
    const protocol = match[1];
    const authority = match[2] || '';
    const host = authority.slice(authority.lastIndexOf('@') + 1);
    const portAt = host.lastIndexOf(':');
    const hasPort = portAt > host.lastIndexOf(']');
    const search = match[4] && match[4] !== '?' ? match[4] : '';
    const query = search.slice(1).split('&').filter(Boolean).map((pair) => {
      const eq = pair.indexOf('=');
      const decode = (part) => { try { return decodeURIComponent(part.replace(/\+/g, ' ')); } catch (_) { return part; } };
      return eq < 0 ? [decode(pair), ''] : [decode(pair.slice(0, eq)), decode(pair.slice(eq + 1))];
    });
    return {
      href,
      protocol,
      origin: authority ? `${protocol}//${host}` : 'null',
      host,
      hostname: hasPort ? host.slice(0, portAt) : host,
      port: hasPort ? host.slice(portAt + 1) : '',
      pathname: match[3] || (authority ? '/' : ''),
      search,
      hash: match[5] && match[5] !== '#' ? match[5] : '',
      searchParams: {
        get: (name) => { const hit = query.find(([key]) => key === name); return hit ? hit[1] : null; },
        getAll: (name) => query.filter(([key]) => key === name).map(([, value]) => value),
        has: (name) => query.some(([key]) => key === name),
      },
      toString: () => href,
    };
  }
  function urlMatches(url, expected) {
    if (expected instanceof RegExp) return expected.test(url);
    if (typeof expected === 'function') return Boolean(expected(parseUrl(url)));
    const pattern = String(expected);
    if (!/[*?{]/.test(pattern)) return url === pattern;
    return new RegExp(globSource(pattern)).test(url);
  }
  // Anchored regex source of a URL glob; also sent to the host, so it stays plain syntax.
  function globSource(pattern) {
    let source = '';
    for (let i = 0; i < pattern.length; i++) {
      const c = pattern[i];
      if (c === '*' && pattern[i + 1] === '*') { source += '.*'; i++; }
      else if (c === '*') source += '[^/]*';
      else if (c === '?') source += '\\?';
      else if (c === '{') source += '(';
      else if (c === '}') source += ')';
      else if (c === ',') source += '|';
      else source += c.replace(/[.+^$()|[\]\\]/g, '\\$&');
    }
    return `^${source}$`;
  }

  class Request {
    constructor(data) { this._data = data; }
    url() { return this._data.url; }
    method() { return this._data.method; }
    resourceType() { return String(this._data.resourceType || '').toLowerCase(); }
    toJSON() { return { url: this.url(), method: this.method(), resourceType: this.resourceType() }; }
  }
  class Response {
    constructor(data) { this._data = data; }
    url() { return this._data.url; }
    status() { return this._data.status; }
    statusText() { return this._data.statusText; }
    ok() { const status = this._data.status; return status === 0 || (status >= 200 && status <= 299); }
    headers() { return Object.assign({}, this._data.headers); }
    async headerValue(name) { const value = this._data.headers[String(name).toLowerCase()]; return value === undefined ? null : value; }
    request() { return strict(new Request(this._data), 'request'); }
    async text() {
      const reply = await call('net_body', { requestId: this._data.requestId });
      if (reply.base64) throw named('NotSupported', 'response.text() is not available for binary bodies');
      return reply.body;
    }
    async json() { return JSON.parse(await this.text()); }
    toJSON() { return { url: this.url(), status: this.status(), statusText: this.statusText(), ok: this.ok() }; }
  }
  const wrapResponse = (data) => (data ? strict(new Response(data), 'response') : null);
  const wrapRequest = (data) => strict(new Request(data), 'request');

  // goto, reload, and history moves resolve with the main document's Response, or null when
  // there was none (a fragment change, a page restored from the cache).
  async function navigate(operation, args, options) {
    return wrapResponse(await call(operation, Object.assign({}, args, { timeout: options.timeout, waitUntil: options.waitUntil })));
  }

  // Only requests and responses that start after the call count, like a Playwright listener
  // registered at call time. The host stamps each log entry, so the filter holds even when an
  // action queued ahead of this wait finishes first (`Promise.all([wait, click])` in either order).
  async function waitForNetwork(kind, api, matcher, options) {
    const startedAt = Date.now();
    const timeout = options.timeout === undefined ? 5000 : options.timeout;
    const deadline = timeout === 0 ? Infinity : startedAt + timeout;
    const wrap = kind === 'response' ? wrapResponse : wrapRequest;
    let after = 0;
    for (;;) {
      const reply = await call('net_events', { after, kinds: [kind] });
      const advanced = reply.cursor > after;
      after = reply.cursor;
      for (const data of reply.events) {
        if (data.at < startedAt) continue;
        const subject = wrap(data);
        const hit = typeof matcher === 'function' ? await matcher(subject) : urlMatches(data.url, matcher);
        if (hit) return subject;
      }
      if (Date.now() >= deadline) {
        throw named('TimeoutError', `${api}: Timeout ${timeout}ms exceeded while waiting for ${typeof matcher === 'function' ? 'a matching ' + kind : quote(matcher)}`);
      }
      if (!advanced) await sleep(50);
    }
  }

  // Page script (ADR-022). The page function is the script's own source wrapped with `codec`, so
  // undefined, NaN, -0, dates, regexps, errors, bigints, shared references, and handles survive
  // the JSON hop in both directions. `codec` is sent to the page as source text: it must not use
  // anything from this closure.
  function codec() {
    function serialize(value, special) {
      const ids = new Map();
      const walk = (v) => {
        if (v === undefined || typeof v === 'function' || typeof v === 'symbol') return { $v: 'undefined' };
        if (v === null || typeof v === 'boolean' || typeof v === 'string') return v;
        if (typeof v === 'number') {
          if (Number.isNaN(v)) return { $v: 'NaN' };
          if (v === Infinity) return { $v: 'Infinity' };
          if (v === -Infinity) return { $v: '-Infinity' };
          return Object.is(v, -0) ? { $v: '-0' } : v;
        }
        if (typeof v === 'bigint') return { $b: String(v) };
        const replaced = special(v);
        if (replaced !== undefined) return replaced;
        if (ids.has(v)) return { $ref: ids.get(v) };
        if (v instanceof Date) return { $d: Number.isNaN(v.getTime()) ? null : v.toISOString() };
        if (v instanceof RegExp) return { $r: { p: v.source, f: v.flags } };
        if (v instanceof Error) return { $e: { n: String(v.name), m: String(v.message), s: String(v.stack || '') } };
        const id = ids.size;
        ids.set(v, id);
        if (Array.isArray(v)) return { $a: Array.from(v, walk), $id: id };
        const out = {};
        for (const key of Object.keys(v)) out[key] = walk(v[key]);
        return { $o: out, $id: id };
      };
      return walk(value);
    }
    function revive(tree, handles) {
      const refs = new Map();
      const walk = (t) => {
        if (t === null || typeof t !== 'object') return t;
        if ('$v' in t) return { undefined: undefined, NaN: NaN, Infinity: Infinity, '-Infinity': -Infinity, '-0': -0 }[t.$v];
        if ('$b' in t) return BigInt(t.$b);
        if ('$d' in t) return new Date(t.$d === null ? NaN : t.$d);
        if ('$r' in t) return new RegExp(t.$r.p, t.$r.f);
        if ('$e' in t) { const error = new Error(t.$e.m); error.name = t.$e.n; error.stack = t.$e.s; return error; }
        if ('$h' in t) return handles[t.$h];
        if ('$ref' in t) return refs.get(t.$ref);
        if ('$a' in t) { const out = []; refs.set(t.$id, out); for (const item of t.$a) out.push(walk(item)); return out; }
        const out = {};
        refs.set(t.$id, out);
        for (const key of Object.keys(t.$o || {})) out[key] = walk(t.$o[key]);
        return out;
      };
      return walk(tree);
    }
    return { serialize, revive };
  }
  const { serialize, revive } = codec();
  // Names are prefixed because the script's source shares their scope. A source that is not a
  // function is an expression; when it evaluates to a function, that function is called.
  function pageFunction(source) {
    return `async function (__openside_spec, __openside_arg, ...__openside_objects) {
  const __openside_codec = (${codec})();
  const __openside_handles = __openside_objects.slice(0, __openside_spec.handles);
  const __openside_input = __openside_codec.revive(__openside_arg, __openside_handles);
  const __openside_target = __openside_spec.target === 'this' ? this
    : __openside_spec.target === 'all' ? __openside_objects.slice(__openside_spec.handles)
    : __openside_codec.revive(__openside_spec.value, __openside_handles);
  const __openside_fn = (
${source}
);
  const __openside_result = typeof __openside_fn !== 'function' ? await __openside_fn
    : __openside_spec.target === 'none' ? await __openside_fn(__openside_input)
    : await __openside_fn(__openside_target, __openside_input);
  if (!__openside_spec.byValue) return __openside_result;
  return __openside_codec.serialize(__openside_result, (v) => (typeof Window === 'function' && v instanceof Window ? 'ref: <Window>'
    : typeof Document === 'function' && v instanceof Document ? 'ref: <Document>'
    : typeof Node === 'function' && v instanceof Node ? 'ref: <Node>' : undefined));
}`;
  }

  // `_remote` is the host's handle descriptor: `object` (or `value` for a primitive) plus the
  // `frame` and `host` session it lives in.
  class JSHandle {
    constructor(remote) {
      this._remote = remote;
      this._disposed = false;
    }
    evaluate(fn, arg) { return runScript('jsHandle.evaluate', fn, arg, { handle: this }); }
    evaluateHandle(fn, arg) { return runScript('jsHandle.evaluateHandle', fn, arg, { handle: this, returnHandle: true }); }
    jsonValue() { return this.evaluate((value) => value); }
    getProperty(name) { return this.evaluateHandle((value, key) => value[key], String(name)); }
    async getProperties() {
      const properties = new Map();
      for (const key of await this.evaluate((value) => Object.keys(value))) properties.set(key, await this.getProperty(key));
      return properties;
    }
    asElement() { return this._remote.subtype === 'node' ? wrapHandle(this) : null; }
    async dispose() {
      if (this._disposed) return;
      this._disposed = true;
      if (this._remote.object) await call('dispose', { object: this._remote.object, host: this._remote.host });
    }
    toString() {
      const remote = this._remote;
      return `JSHandle@${remote.object ? remote.description || remote.subtype || remote.type : show(revive(remote.value, []))}`;
    }
    toJSON() { return this.toString(); }
  }
  function wrapHandle(handle) { return strict(handle, 'jsHandle'); }

  // `handle`: run against a JSHandle; `locator` + `mode` (`strict` like locator actions, `first`
  // for $eval, `all`): run against its elements; neither: run in the page or in `frame` (frame
  // steps).
  async function runScript(api, fn, arg, { handle, locator, mode, timeout, frame, returnHandle = false } = {}) {
    let source;
    if (typeof fn === 'function') source = String(fn);
    else if (typeof fn === 'string') source = fn;
    else throw named('TypeError', `${api}: expected a function or an expression string`);
    const objects = [];
    const tree = serialize(arg, (value) => {
      if (value instanceof JSHandle) {
        if (value._disposed) throw named('Error', `${api}: JSHandle is disposed`);
        if (!value._remote.object) return value._remote.value;
        objects.push({ object: value._remote.object, frame: value._remote.frame, host: value._remote.host });
        return { $h: objects.length - 1 };
      }
      if (value instanceof Locator) throw named('TypeError', `${api}: a locator cannot be passed into the page; use locator.evaluate`);
      return undefined;
    });
    const spec = { handles: objects.length, byValue: !returnHandle, target: 'none' };
    let target = { kind: 'page' };
    if (handle) {
      if (handle._disposed) throw named('Error', `${api}: JSHandle is disposed`);
      const remote = handle._remote;
      if (remote.object) { spec.target = 'this'; target = { kind: 'handle', object: remote.object, frame: remote.frame, host: remote.host }; }
      else { spec.target = 'value'; spec.value = remote.value; frame = remote.frame ? [{ kind: 'frame_id', id: remote.frame }] : null; }
    } else if (locator) {
      spec.target = mode === 'all' ? 'all' : 'this';
      target = Object.assign(locator._args({ timeout }), { kind: 'locator', mode });
    }
    const value = await call('evaluate', { api, declaration: pageFunction(source), spec, arg: tree, handles: objects, target, frame: frame || null, returnHandle });
    return returnHandle ? wrapHandle(new JSHandle(value)) : revive(value, []);
  }
  Locator.prototype.evaluate = function (fn, arg, options = {}) {
    return runScript('locator.evaluate', fn, arg, { locator: this, mode: 'strict', timeout: options.timeout });
  };
  Locator.prototype.evaluateHandle = function (fn, arg, options = {}) {
    return runScript('locator.evaluateHandle', fn, arg, { locator: this, mode: 'strict', timeout: options.timeout, returnHandle: true });
  };
  Locator.prototype.evaluateAll = function (fn, arg) {
    return runScript('locator.evaluateAll', fn, arg, { locator: this, mode: 'all' });
  };
  Frame.prototype.evaluate = function (fn, arg) { return runScript('frame.evaluate', fn, arg, { frame: this._steps }); };
  Frame.prototype.evaluateHandle = function (fn, arg) { return runScript('frame.evaluateHandle', fn, arg, { frame: this._steps, returnHandle: true }); };
  Frame.prototype.$eval = function (selector, fn, arg) { return runScript('frame.$eval', fn, arg, { locator: this._root.locator(selector), mode: 'first' }); };
  Frame.prototype.$$eval = function (selector, fn, arg) { return runScript('frame.$$eval', fn, arg, { locator: this._root.locator(selector), mode: 'all' }); };
  const pageScript = {
    evaluate: (fn, arg) => runScript('page.evaluate', fn, arg),
    evaluateHandle: (fn, arg) => runScript('page.evaluateHandle', fn, arg, { returnHandle: true }),
    $eval: (selector, fn, arg) => runScript('page.$eval', fn, arg, { locator: rootLocator(selector), mode: 'first' }),
    $$eval: (selector, fn, arg) => runScript('page.$$eval', fn, arg, { locator: rootLocator(selector), mode: 'all' }),
    // Init scripts are removed when this execution ends.
    addInitScript: async (script, arg) => {
      let source;
      if (typeof script === 'function') source = `(${String(script)})(${arg === undefined ? '' : JSON.stringify(arg)})`;
      else if (typeof script === 'string') source = script;
      else if (script && typeof script.content === 'string') source = script.content;
      else if (script && script.path !== undefined) unsupported('page.addInitScript({ path })', 'no filesystem access');
      else throw named('TypeError', 'page.addInitScript: expected a function, a string, or { content }');
      await call('add_init_script', { source });
    },
    cdp: (method, params) => call('cdp', { method: String(method), params: params === undefined ? null : params }),
  };


  // Page events (dialog, popup, download, filechooser). One pump pulls events from the host while
  // a listener or waiter exists; the host calls it makes bypass the operation queue, because a
  // click that opens a dialog does not return until a handler has answered it.
  const EVENT_NAMES = ['dialog', 'popup', 'download', 'filechooser'];
  const EVENT_SLICE_MS = 50;
  const events = { listeners: [], waiters: [], pump: null, closing: false, used: false };
  const interested = (kind) => events.listeners.some((l) => l.kind === kind) || events.waiters.some((w) => w.kind === kind);
  function checkEvent(api, kind) {
    if (!EVENT_NAMES.includes(kind)) unsupported(`${api}(${JSON.stringify(kind)})`, `supported events: ${EVENT_NAMES.join(', ')}`);
  }
  function fileList(files) {
    const list = Array.isArray(files) ? files : [files];
    if (list.some((file) => typeof file !== 'string')) unsupported('setInputFiles with in-memory files', 'pass absolute file paths');
    return list;
  }
  class Dialog {
    constructor(raw) { this._raw = raw; this._done = false; }
    type() { return this._raw.type; }
    message() { return this._raw.message; }
    defaultValue() { return this._raw.default_value; }
    page() { return page; }
    accept(promptText) { return this._answer(true, promptText === undefined ? '' : String(promptText)); }
    dismiss() { return this._answer(false, ''); }
    async _answer(accept, text) {
      if (this._done) throw named('Error', `Cannot ${accept ? 'accept' : 'dismiss'} dialog which is already handled!`);
      this._done = true;
      await call('dialog_handle', { accept, text });
    }
  }
  // A popup is a page of the same session that the script may read but not drive.
  class Popup {
    constructor(info) { this._info = info; }
    get tabId() { return this._info.tab_id; }
    url() { return this._info.url; }
    async title() { await this._refresh(false); return this._info.title; }
    async waitForLoadState() { await this._refresh(true); return null; }
    async _refresh(settle) { this._info = await call('popup_info', { tab_id: this._info.tab_id, settle }); }
    opener() { return page; }
  }
  class Download {
    constructor(raw) { this._raw = raw; this._path = null; }
    url() { return this._raw.url; }
    suggestedFilename() { return this._raw.suggested_filename; }
    page() { return page; }
    async saveAs(name, options = {}) {
      const reply = await call('download_save', { guid: this._raw.guid, name: name === undefined ? null : String(name), timeout: options.timeout });
      this._path = reply.path;
      return reply.path;
    }
    async path(options) { return this._path || this.saveAs(undefined, options); }
    failure(options = {}) { return call('download_failure', { guid: this._raw.guid, timeout: options.timeout }); }
  }
  class FileChooser {
    constructor(raw) { this._raw = raw; }
    isMultiple() { return !!this._raw.multiple; }
    page() { return page; }
    setFiles(files) { return call('chooser_set_files', { chooser_id: this._raw.chooser_id, files: fileList(files) }); }
  }
  function eventSubject(raw) {
    if (raw.kind === 'dialog') return strict(new Dialog(raw), 'dialog');
    if (raw.kind === 'popup') return strict(new Popup({ tab_id: raw.tab_id, url: raw.url || '' }), 'popup');
    if (raw.kind === 'download') return strict(new Download(raw), 'download');
    return strict(new FileChooser(raw), 'fileChooser');
  }
  function syncEvents() {
    events.used = true;
    // Host calls run in the order issued, so interest declared before an action is in place
    // before the action starts.
    call('events_set', { dialog: interested('dialog'), filechooser: interested('filechooser') })
      .catch((error) => log(`[page events] ${describe(error)}`));
    if (!events.pump && !events.closing && EVENT_NAMES.some(interested)) {
      events.pump = runPump();
    }
  }
  function expireWaiters() {
    const now = Date.now();
    const expired = events.waiters.filter((w) => w.deadline <= now);
    if (!expired.length) return;
    events.waiters = events.waiters.filter((w) => w.deadline > now);
    for (const w of expired) w.reject(named('TimeoutError', `page.waitForEvent: Timeout ${w.timeout}ms exceeded while waiting for event ${JSON.stringify(w.kind)}`));
    syncEvents();
  }
  async function deliver(raw) {
    const subject = eventSubject(raw);
    if (raw.kind === 'popup') await subject._refresh(true);
    let held = false;
    for (const waiter of events.waiters.filter((w) => w.kind === raw.kind)) {
      let match = true;
      try { match = waiter.predicate ? !!(await waiter.predicate(subject)) : true; } catch (error) {
        events.waiters = events.waiters.filter((w) => w !== waiter);
        waiter.reject(error);
        continue;
      }
      if (match) {
        events.waiters = events.waiters.filter((w) => w !== waiter);
        waiter.resolve(subject);
        held = true;
      }
    }
    for (const listener of events.listeners.filter((l) => l.kind === raw.kind)) {
      if (listener.once) events.listeners = events.listeners.filter((l) => l !== listener);
      try { await listener.handler(subject); } catch (error) { log(`[page.on(${JSON.stringify(raw.kind)})] ${describe(error)}`); }
    }
    // A dialog nobody answered would freeze the page: answer it as the daemon does without a
    // script (accept alerts and leave-page prompts, cancel confirms and prompts). A waiter that
    // received it owns the answer.
    if (raw.kind === 'dialog' && !held && !subject._done) {
      try { await (raw.type === 'alert' || raw.type === 'beforeunload' ? subject.accept() : subject.dismiss()); } catch (_) { /* already gone */ }
    }
    syncEvents();
  }
  async function runPump() {
    try {
      while (!events.closing && EVENT_NAMES.some(interested)) {
        // A waiter added while this call is pending must still time out on schedule, so the call
        // never outlasts a short slice.
        const deadlines = events.waiters.map((w) => w.deadline).filter((d) => d !== Infinity);
        const slice = deadlines.length ? Math.min(EVENT_SLICE_MS, Math.max(1, Math.min(...deadlines) - Date.now())) : EVENT_SLICE_MS;
        const raw = await call('event_next', { timeout: slice });
        if (raw) await deliver(raw);
        expireWaiters();
      }
    } catch (error) {
      const waiters = events.waiters;
      events.waiters = [];
      for (const w of waiters) w.reject(error);
    } finally {
      // Cleared in the step that decided to stop, so a waiter added right after cannot find a
      // pump that is already leaving and start none.
      events.pump = null;
    }
  }
  function addListener(kind, handler, once) {
    checkEvent(once ? 'page.once' : 'page.on', kind);
    if (typeof handler !== 'function') throw named('TypeError', `page.${once ? 'once' : 'on'}: handler must be a function`);
    events.listeners.push({ kind, handler, once });
    syncEvents();
    return page;
  }
  function removeListener(kind, handler) {
    checkEvent('page.off', kind);
    const index = events.listeners.findIndex((l) => l.kind === kind && l.handler === handler);
    if (index >= 0) events.listeners.splice(index, 1);
    syncEvents();
    return page;
  }
  function waitForEvent(kind, options) {
    checkEvent('page.waitForEvent', kind);
    const opts = typeof options === 'function' ? { predicate: options } : options || {};
    const timeout = opts.timeout === undefined ? 5000 : Number(opts.timeout);
    return new Promise((resolve, reject) => {
      events.waiters.push({ kind, predicate: opts.predicate, resolve, reject, timeout, deadline: timeout === 0 ? Infinity : Date.now() + timeout });
      syncEvents();
    });
  }
  async function closeEvents() {
    events.closing = true;
    const pump = events.pump;
    if (!events.used) return;
    let closed = false;
    try { await call('events_close', {}); closed = true; } catch (_) { /* the execution was stopped; the host releases its side */ }
    if (closed && pump) await pump;
  }
  const keyboard = strict({
    press: (key, options = {}) => call('keyboard', { action: 'press', text: String(key), delay: options.delay }),
    down: (key) => call('keyboard', { action: 'down', text: String(key) }),
    up: (key) => call('keyboard', { action: 'up', text: String(key) }),
    type: (value, options = {}) => call('keyboard', { action: 'type', text: String(value), delay: options.delay, ime: options.ime }),
    insertText: (value) => call('keyboard', { action: 'insertText', text: String(value) }),
    imeSetComposition: (value, options = {}) => call('keyboard', { action: 'imeSetComposition', text: String(value), selectionStart: options.selectionStart, selectionEnd: options.selectionEnd }),
  }, 'page.keyboard');
  const mouseCall = (action, extra) => call('mouse', Object.assign({ action }, extra));
  const mouse = strict({
    move: (x, y, options = {}) => mouseCall('move', { x: Number(x), y: Number(y), steps: options.steps }),
    down: (options = {}) => mouseCall('down', { button: options.button, clickCount: options.clickCount }),
    up: (options = {}) => mouseCall('up', { button: options.button, clickCount: options.clickCount }),
    click: (x, y, options = {}) => mouseCall('click', { x: Number(x), y: Number(y), button: options.button, clickCount: options.clickCount, delay: options.delay }),
    dblclick: (x, y, options = {}) => mouseCall('click', { x: Number(x), y: Number(y), button: options.button, clickCount: 2, delay: options.delay }),
    wheel: (deltaX, deltaY) => mouseCall('wheel', { deltaX: Number(deltaX), deltaY: Number(deltaY) }),
  }, 'page.mouse');

  const page = strict({
    goto: (url, options = {}) => navigate('goto', { url: String(url) }, options),
    reload: (options = {}) => navigate('reload', {}, options),
    goBack: (options = {}) => navigate('history', { delta: -1 }, options),
    goForward: (options = {}) => navigate('history', { delta: 1 }, options),
    url: () => state.url,
    title: () => call('title'),
    content: () => call('content'),
    snapshot: () => call('snapshot'),
    waitForTimeout: (ms) => sleep(Number(ms)),
    waitForLoadState: (loadState = 'load', options = {}) => call('load_state', { state: loadState, timeout: options.timeout }),
    waitForURL: async (expected, options = {}) => {
      const timeout = options.timeout === undefined ? 5000 : options.timeout;
      const startedAt = Date.now();
      const deadline = timeout === 0 ? Infinity : startedAt + timeout;
      // The first host call starts tracking, so navigations from here on are recorded.
      let after = (await call('net_events', {})).cursor;
      let url = await call('url');
      for (;;) {
        let matched = urlMatches(url, expected);
        if (!matched) {
          // A navigation can come and go between two polls; the host keeps every one.
          const reply = await call('net_events', { after, kinds: ['navigated'] });
          after = reply.cursor;
          matched = reply.events.some((event) => event.at >= startedAt && urlMatches(event.url, expected));
        }
        if (matched) {
          if (options.waitUntil === 'commit') return null;
          return call('load_state', { state: options.waitUntil || 'load', timeout: deadline === Infinity ? undefined : Math.max(1, deadline - Date.now()) });
        }
        if (Date.now() >= deadline) throw named('TimeoutError', `page.waitForURL: Timeout ${timeout}ms exceeded waiting for ${quote(expected)}; current URL ${url}`);
        await sleep(50);
        url = await call('url');
      }
    },
    waitForResponse: (matcher, options = {}) => waitForNetwork('response', 'page.waitForResponse', matcher, options),
    waitForRequest: (matcher, options = {}) => waitForNetwork('request', 'page.waitForRequest', matcher, options),
    waitForFunction: async (fn, arg, options = {}) => {
      const timeout = options.timeout === undefined ? 5000 : options.timeout;
      const interval = typeof options.polling === 'number' && options.polling > 0 ? options.polling : 50;
      const deadline = timeout === 0 ? Infinity : Date.now() + timeout;
      for (;;) {
        const handle = await runScript('page.waitForFunction', fn, arg, { returnHandle: true });
        if (await handle.evaluate((value) => Boolean(value))) return handle;
        await handle.dispose();
        if (Date.now() >= deadline) throw named('TimeoutError', `page.waitForFunction: Timeout ${timeout}ms exceeded`);
        await sleep(interval);
      }
    },
    waitForSelector: async (selector, options = {}) => { await rootLocator(selector).waitFor(options); return null; },
    locator: rootLocator,
    getByRole: (...args) => root.getByRole(...args),
    getByText: (...args) => root.getByText(...args),
    getByLabel: (...args) => root.getByLabel(...args),
    getByPlaceholder: (...args) => root.getByPlaceholder(...args),
    getByAltText: (...args) => root.getByAltText(...args),
    getByTitle: (...args) => root.getByTitle(...args),
    getByTestId: (...args) => root.getByTestId(...args),
    mainFrame: () => mainFrame,
    frames: pageFrames,
    frame: pageFrame,
    frameLocator: (selector) => new FrameLocator(rootLocator(selector), `page.frameLocator(${quote(selector)})`),
    click: delegate('click'), dblclick: delegate('dblclick'), fill: delegate('fill'), type: delegate('type'),
    press: delegate('press'), check: delegate('check'), uncheck: delegate('uncheck'), hover: delegate('hover'),
    focus: delegate('focus'), selectOption: delegate('selectOption'), textContent: delegate('textContent'),
    innerText: delegate('innerText'), getAttribute: delegate('getAttribute'), inputValue: delegate('inputValue'),
    on: (kind, handler) => addListener(kind, handler, false), once: (kind, handler) => addListener(kind, handler, true),
    off: removeListener, removeListener, waitForEvent, setInputFiles: delegate('setInputFiles'),
    isVisible: delegate('isVisible'), isHidden: delegate('isHidden'), isChecked: delegate('isChecked'),
    isEnabled: delegate('isEnabled'), isDisabled: delegate('isDisabled'), isEditable: delegate('isEditable'),
    keyboard, mouse,
    dragAndDrop: (source, target, options = {}) => rootLocator(source).dragTo(rootLocator(target), options),
    ...pageScript,
  }, 'page');

  function show(value) {
    if (typeof value === 'string') return value;
    if (value instanceof Error) return `${value.name}: ${value.message}`;
    if (value instanceof Locator) return value.toString();
    try { const json = JSON.stringify(value); return json === undefined ? String(value) : json; } catch (_) { return String(value); }
  }
  function equal(a, b) {
    if (Object.is(a, b)) return true;
    if (typeof a !== 'object' || typeof b !== 'object' || a === null || b === null) return false;
    if (Array.isArray(a) !== Array.isArray(b)) return false;
    const keys = Object.keys(a);
    if (keys.length !== Object.keys(b).length) return false;
    return keys.every((key) => equal(a[key], b[key]));
  }
  const normalizeText = (value) => String(value == null ? '' : value).replace(/\s+/g, ' ').trim();
  function textMatches(actual, expected, contain, ignoreCase) {
    if (expected instanceof RegExp) return expected.test(actual);
    let a = normalizeText(actual), e = normalizeText(expected);
    if (ignoreCase) { a = a.toLowerCase(); e = e.toLowerCase(); }
    return contain ? a.includes(e) : a === e;
  }

  function assertion(subject, negated, timeout) {
    const label = subject instanceof Locator ? 'locator' : subject === page ? 'page' : 'value';
    function fail(matcher, expected, received) {
      const lines = [`expect(${label})${negated ? '.not' : ''}.${matcher} failed`];
      if (subject instanceof Locator) lines.push(`Locator: ${subject.toString()}`);
      if (expected !== undefined) lines.push(`Expected: ${show(expected)}`);
      lines.push(`Received: ${show(received)}`);
      throw named('Error', lines.join('\n'));
    }
    async function poll(matcher, expected, probe, options = {}) {
      const wait = options.timeout || timeout;
      const deadline = Date.now() + wait;
      let received;
      for (;;) {
        let ok;
        try { const result = await probe(); ok = result.ok; received = result.received; }
        catch (error) { if (error && error.name === 'PolicyError') throw error; ok = false; received = show(error); }
        if (ok !== negated) return;
        if (Date.now() >= deadline) fail(`${matcher} (timeout ${wait}ms)`, expected, received);
        await sleep(100);
      }
    }
    function sync(matcher, ok, expected) {
      if (ok === negated) fail(matcher, expected, subject);
    }
    const locatorText = (inner) => async () => {
      const texts = await subject.allTextContents();
      if (texts.length !== 1) return { ok: false, received: texts.length === 0 ? '<element(s) not found>' : `${texts.length} elements` };
      return { value: inner ? normalizeText(texts[0]) : texts[0] };
    };
    const matchers = {
      // Locator assertions (polling).
      toBeVisible: (options) => poll('toBeVisible()', undefined, async () => { const v = await subject.isVisible(); return { ok: v, received: v ? 'visible' : 'hidden' }; }, options),
      toBeHidden: (options) => poll('toBeHidden()', undefined, async () => { const v = await subject.isHidden(); return { ok: v, received: v ? 'hidden' : 'visible' }; }, options),
      toBeChecked: (options = {}) => poll('toBeChecked()', undefined, async () => { const v = await subject.isChecked({ timeout: 1 }); return { ok: v === (options.checked !== false), received: v ? 'checked' : 'unchecked' }; }, options),
      toBeEnabled: (options) => poll('toBeEnabled()', undefined, async () => { const v = await subject.isEnabled({ timeout: 1 }); return { ok: v, received: v ? 'enabled' : 'disabled' }; }, options),
      toBeDisabled: (options) => poll('toBeDisabled()', undefined, async () => { const v = await subject.isDisabled({ timeout: 1 }); return { ok: v, received: v ? 'disabled' : 'enabled' }; }, options),
      toBeEditable: (options) => poll('toBeEditable()', undefined, async () => { const v = await subject.isEditable({ timeout: 1 }); return { ok: v, received: v ? 'editable' : 'readonly' }; }, options),
      toBeFocused: (options) => poll('toBeFocused()', undefined, async () => { const v = await subject._focused({ timeout: 1 }); return { ok: v, received: v ? 'focused' : 'not focused' }; }, options),
      toBeAttached: (options) => poll('toBeAttached()', undefined, async () => { const n = await subject.count(); return { ok: n > 0, received: `${n} elements` }; }, options),
      toHaveCount: (expected, options) => poll('toHaveCount()', expected, async () => { const n = await subject.count(); return { ok: n === expected, received: n }; }, options),
      toHaveText: (expected, options = {}) => poll('toHaveText()', expected, async () => {
        if (Array.isArray(expected)) { const texts = await subject.allTextContents(); return { ok: texts.length === expected.length && texts.every((t, i) => textMatches(t, expected[i], false, options.ignoreCase)), received: texts }; }
        const r = await locatorText(false)(); if (r.ok === false) return r; return { ok: textMatches(r.value, expected, false, options.ignoreCase), received: r.value };
      }, options),
      toContainText: (expected, options = {}) => poll('toContainText()', expected, async () => {
        const r = await locatorText(false)(); if (r.ok === false) return r; return { ok: textMatches(r.value, expected, true, options.ignoreCase), received: r.value };
      }, options),
      toHaveValue: (expected, options) => poll('toHaveValue()', expected, async () => { const v = await subject.inputValue({ timeout: 1 }); return { ok: expected instanceof RegExp ? expected.test(v) : v === String(expected), received: v }; }, options),
      toHaveAttribute: (name, expected, options) => poll('toHaveAttribute()', expected, async () => {
        const v = await subject.getAttribute(name, { timeout: 1 });
        const ok = expected === undefined ? v !== null : expected instanceof RegExp ? v !== null && expected.test(v) : v === String(expected);
        return { ok, received: v };
      }, options),
      // Page assertions (polling).
      toHaveURL: (expected, options) => poll('toHaveURL()', expected, async () => { const url = await call('url'); return { ok: urlMatches(url, expected), received: url }; }, options),
      toHaveTitle: (expected, options) => poll('toHaveTitle()', expected, async () => { const title = await page.title(); return { ok: textMatches(title, expected, false), received: title }; }, options),
      // Generic value assertions (immediate).
      toBe: (expected) => sync('toBe()', Object.is(subject, expected), expected),
      toEqual: (expected) => sync('toEqual()', equal(subject, expected), expected),
      toStrictEqual: (expected) => sync('toStrictEqual()', equal(subject, expected), expected),
      toBeTruthy: () => sync('toBeTruthy()', !!subject),
      toBeFalsy: () => sync('toBeFalsy()', !subject),
      toBeNull: () => sync('toBeNull()', subject === null),
      toBeUndefined: () => sync('toBeUndefined()', subject === undefined),
      toBeDefined: () => sync('toBeDefined()', subject !== undefined),
      toContain: (expected) => sync('toContain()', subject != null && subject.includes(expected), expected),
      toHaveLength: (expected) => sync('toHaveLength()', subject != null && subject.length === expected, expected),
      toMatch: (expected) => sync('toMatch()', typeof subject === 'string' && (expected instanceof RegExp ? expected.test(subject) : subject.includes(expected)), expected),
      toBeGreaterThan: (expected) => sync('toBeGreaterThan()', subject > expected, expected),
      toBeGreaterThanOrEqual: (expected) => sync('toBeGreaterThanOrEqual()', subject >= expected, expected),
      toBeLessThan: (expected) => sync('toBeLessThan()', subject < expected, expected),
      toBeLessThanOrEqual: (expected) => sync('toBeLessThanOrEqual()', subject <= expected, expected),
    };
    return matchers;
  }
  function makeExpect(subject, timeout) {
    const unwrapped = subject;
    const base = assertion(unwrapped, false, timeout);
    base.not = strict(assertion(unwrapped, true, timeout), 'expect().not');
    return strict(base, 'expect()');
  }
  function expect(subject, options = {}) {
    return makeExpect(subject, options.timeout || 5000);
  }
  expect.soft = () => unsupported('expect.soft', 'every failed assertion throws');
  expect.poll = () => unsupported('expect.poll');

  const consoleObject = {};
  for (const level of ['log', 'info', 'warn', 'error', 'debug']) {
    consoleObject[level] = (...args) => log((level === 'log' || level === 'info' ? '' : `[${level}] `) + args.map(show).join(' '));
  }
  globalThis.console = consoleObject;
  globalThis.page = page;
  globalThis.expect = expect;

  function describe(error) {
    if (error && typeof error === 'object' && 'message' in error) return `${error.name || 'Error'}: ${error.message}`;
    return `Uncaught ${show(error)}`;
  }
  function toJson(value) {
    if (value === undefined) return null;
    try {
      const json = JSON.stringify(value, (_, item) => (typeof item === 'bigint' ? String(item) : item));
      return json === undefined ? null : JSON.parse(json);
    } catch (_) {
      return String(value);
    }
  }
  async function finish(promise) {
    let reply;
    try {
      const completion = await promise;
      // A trailing `page.title()` without `await` is a promise; report what it resolves to.
      const value = await (completion && completion.value);
      reply = JSON.stringify({ ok: true, value: toJson(value) });
    } catch (error) {
      reply = JSON.stringify({ ok: false, error: describe(error) });
    }
    await closeEvents();
    return reply;
  }
  return { finish, describe };
})()
