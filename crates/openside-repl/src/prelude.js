// Playwright-compatible prelude. Runs only inside QuickJS; every page operation is a call to the
// Rust host (`__openside_op`), which talks CDP DOM/Accessibility/Input/Page. Nothing here is ever
// sent to the page.
(() => {
  'use strict';
  const op = globalThis.__openside_op;
  const log = globalThis.__openside_log;
  const state = { url: String(globalThis.__openside_url || '') };
  delete globalThis.__openside_op;
  delete globalThis.__openside_log;
  delete globalThis.__openside_url;

  function named(name, message) {
    const error = new Error(message);
    error.name = name;
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

  // Known Playwright APIs that the CDP-only design cannot offer get a reason; anything else unknown
  // still throws NotSupported instead of silently returning undefined.
  const REASONS = {
    evaluate: 'page scripts are never injected', evaluateHandle: 'page scripts are never injected',
    $eval: 'page scripts are never injected', $$eval: 'page scripts are never injected',
    addInitScript: 'page scripts are never injected', addScriptTag: 'page scripts are never injected',
    addStyleTag: 'page scripts are never injected', exposeFunction: 'page scripts are never injected',
    exposeBinding: 'page scripts are never injected', route: 'no network interception',
    unroute: 'no network interception', request: 'no raw network access', context: 'one tab per REPL call',
    screenshot: 'use the browser_screenshot tool', pdf: 'not available', setContent: 'navigate with page.goto',
    evaluateAll: 'page scripts are never injected', dispatchEvent: 'page scripts are never injected',
    setInputFiles: 'no filesystem access', elementHandle: 'element handles need page scripts',
    elementHandles: 'element handles need page scripts', $: 'element handles need page scripts',
    $$: 'element handles need page scripts', frameLocator: 'iframes are outside the top-frame locator scope',
    mouse: 'use locator actions so policy sees the target', waitForFunction: 'page scripts are never injected',
    waitForResponse: 'no network observation', waitForRequest: 'no network observation',
    waitForEvent: 'no event subscription', on: 'no event subscription', once: 'no event subscription',
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
      } else if (engine === 'xpath' || part.startsWith('//') || part.startsWith('..')) {
        unsupported('XPath selectors', 'use CSS, role, text, or aria-ref selectors');
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
  }
  function wrapLocator(locator) { return strict(locator, 'locator'); }
  const root = new Locator([], 'page');
  const rootLocator = (selector, options) => root.locator(selector, options);
  const delegate = (method) => (selector, ...args) => rootLocator(selector)[method](...args);

  function urlMatches(url, expected) {
    if (expected instanceof RegExp) return expected.test(url);
    if (typeof expected === 'function') unsupported('URL predicates', 'the sandbox has no URL class');
    const pattern = String(expected);
    if (!/[*?{]/.test(pattern)) return url === pattern;
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
    return new RegExp(`^${source}$`).test(url);
  }

  const keyboard = strict({
    press: (key, options = {}) => call('keyboard', { action: 'press', text: String(key), delay: options.delay }),
    type: (value, options = {}) => call('keyboard', { action: 'type', text: String(value), delay: options.delay }),
    insertText: (value) => call('keyboard', { action: 'insertText', text: String(value) }),
  }, 'page.keyboard');

  const page = strict({
    goto: async (url, options = {}) => { await call('goto', { url: String(url), timeout: options.timeout }); return null; },
    reload: async (options = {}) => { await call('reload', { timeout: options.timeout }); return null; },
    goBack: async (options = {}) => { await call('history', { delta: -1, timeout: options.timeout }); return null; },
    goForward: async (options = {}) => { await call('history', { delta: 1, timeout: options.timeout }); return null; },
    url: () => state.url,
    title: () => call('title'),
    content: () => call('content'),
    snapshot: () => call('snapshot'),
    waitForTimeout: (ms) => sleep(Number(ms)),
    waitForLoadState: (loadState = 'load', options = {}) => call('load_state', { state: loadState, timeout: options.timeout }),
    waitForURL: async (expected, options = {}) => {
      const deadline = Date.now() + (options.timeout || 5000);
      for (;;) {
        const url = await call('url');
        if (urlMatches(url, expected)) return call('load_state', { state: options.waitUntil || 'load' });
        if (Date.now() >= deadline) throw named('TimeoutError', `page.waitForURL: Timeout ${options.timeout || 5000}ms exceeded waiting for ${quote(expected)}; current URL ${url}`);
        await sleep(100);
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
    click: delegate('click'), dblclick: delegate('dblclick'), fill: delegate('fill'), type: delegate('type'),
    press: delegate('press'), check: delegate('check'), uncheck: delegate('uncheck'), hover: delegate('hover'),
    focus: delegate('focus'), selectOption: delegate('selectOption'), textContent: delegate('textContent'),
    innerText: delegate('innerText'), getAttribute: delegate('getAttribute'), inputValue: delegate('inputValue'),
    isVisible: delegate('isVisible'), isHidden: delegate('isHidden'), isChecked: delegate('isChecked'),
    isEnabled: delegate('isEnabled'), isDisabled: delegate('isDisabled'), isEditable: delegate('isEditable'),
    keyboard,
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
    try {
      const completion = await promise;
      return JSON.stringify({ ok: true, value: toJson(completion && completion.value) });
    } catch (error) {
      return JSON.stringify({ ok: false, error: describe(error) });
    }
  }
  return { finish, describe };
})()
