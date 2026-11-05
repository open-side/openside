// Globals of a persistent REPL context. Each attached tab gets its own prelude instance, so
// locators and `expect` stay bound to the tab they came from; `page` starts unset until a tab
// is attached. Host operations take the tab id first; the empty id addresses the context.
((makeInstance, op, log) => {
  'use strict';
  const instances = new Map();
  const neutral = makeInstance((name, payload) => op('', name, payload), log, '');
  function named(name, message) {
    const error = new Error(message);
    error.name = name;
    return error;
  }
  async function call(name, args) {
    const reply = JSON.parse(await op('', name, JSON.stringify(args === undefined ? {} : args)));
    if (reply.error !== undefined) throw named(reply.name || 'Error', reply.error);
    return reply.value;
  }
  function attach(tab, url) {
    let instance = instances.get(tab);
    if (instance) instance.begin(url);
    else {
      instance = makeInstance((name, payload) => op(tab, name, payload), log, url);
      instances.set(tab, instance);
    }
    return instance.page;
  }
  function tabOf(value) {
    for (const [tab, instance] of instances) if (instance.page === value) return tab;
    return null;
  }
  // Tabs the host no longer has (closed, or detached by the daemon) drop out here.
  function sync(tabs) {
    const live = new Set(tabs.map((tab) => tab.tab_id));
    for (const tab of [...instances.keys()]) if (!live.has(tab)) instances.delete(tab);
    if (globalThis.page !== undefined && tabOf(globalThis.page) === null) globalThis.page = undefined;
    return tabs.map((tab) => instances.has(tab.tab_id) ? instances.get(tab.tab_id).page : attach(tab.tab_id, tab.url));
  }

  const expect = (subject, options) => {
    for (const instance of instances.values()) if (instance.owns(subject)) return instance.expect(subject, options);
    return neutral.expect(subject, options);
  };
  expect.soft = neutral.expect.soft;
  expect.poll = neutral.expect.poll;
  globalThis.console = neutral.console;
  globalThis.expect = expect;
  globalThis.page = undefined;
  globalThis.openTab = async (url) => {
    const opened = await call('open_tab');
    const page = attach(opened.tab_id, opened.url);
    globalThis.page = page;
    if (url !== undefined && String(url) !== 'about:blank') await page.goto(String(url));
    return page;
  };
  globalThis.getTabs = async () => sync(await call('tabs'));

  // `tabs` are the attached tabs with their current URLs; `select` becomes `page`.
  function start(tabs, select) {
    for (const tab of tabs) attach(tab.tab_id, tab.url);
    sync(tabs);
    if (select) globalThis.page = instances.get(select).page;
  }
  async function finish(promise) {
    let reply;
    try {
      const completion = await promise;
      const value = await (completion && completion.value);
      reply = { ok: true, value: neutral.toJson(value) };
    } catch (error) {
      reply = { ok: false, error: neutral.describe(error) };
    }
    for (const instance of instances.values()) await instance.closeEvents();
    reply.page = globalThis.page === undefined ? null : tabOf(globalThis.page);
    return JSON.stringify(reply);
  }
  return { start, finish, describe: neutral.describe };
})
