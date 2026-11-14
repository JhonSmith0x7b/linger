import { pageAction, playbackConfirmed, searchConfirmed } from './page.js';
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms));
let busy = false;
let loopRunning = false;
let timer;
let lastConnection = '';
const actions = new Set(['search', 'play', 'pause', 'resume', 'next', 'status', 'queue']);
const errorText = error => String(error?.message || error).slice(0, 500);
const extensionVersion = chrome.runtime.getManifest().version;
async function settings() { return (await chrome.storage.local.get('settings')).settings; }
async function note(event, detail = {}) {
  const { logs = [] } = await chrome.storage.local.get('logs');
  logs.push(`${new Date().toISOString()} ${event} ${JSON.stringify(detail)}`);
  await chrome.storage.local.set({ logs: logs.slice(-60) });
}
async function connection(value) {
  if (value === lastConnection) return;
  lastConnection = value;
  await chrome.storage.local.set({ connection: value });
}
async function bridge(config, path, body) {
  const response = await fetch(`http://127.0.0.1:${config.port}${path}`, {
    method: body ? 'POST' : 'GET',
    headers: { Authorization: `Bearer ${config.token}`, 'X-Linger-Client': chrome.runtime.id,
      ...(body ? { 'Content-Type': 'application/json' } : {}) },
    ...(body ? { body: JSON.stringify(body) } : {}), signal: AbortSignal.timeout(4000),
  });
  if (!response.ok) throw Error(response.status === 401 || response.status === 403
    ? '配对失效或已有其他扩展连接，请重新复制 Linger 配对码' : `Linger 连接 HTTP ${response.status}`);
  return response.json();
}
async function page(tabId, action, args = {}, deadline = Date.now() + 25000) {
  if (Date.now() >= deadline) throw Error('音乐命令已过期，未继续执行');
  const tab = await chrome.tabs.get(tabId).catch(() => null);
  if (!tab || !tab.url?.startsWith('https://music.youtube.com/')) throw Error('绑定的音乐标签页已关闭或离开 YouTube Music，请重新绑定');
  const result = await Promise.race([
    chrome.scripting.executeScript({ target: { tabId }, world: 'MAIN', func: pageAction, args: [action, { ...args, __deadline_ms: deadline }] }),
    sleep(Math.max(1, Math.min(8000, deadline - Date.now()))).then(() => { throw Error('音乐页面响应超时，结果未知；请先查询状态'); }),
  ]);
  if (result[0]?.error) throw Error(result[0].error.message || '页面操作失败');
  if (!result[0]?.result) throw Error('音乐页面未返回结果，请检查页面是否已加载');
  return result[0].result;
}
async function until(tabId, action, args, accept, seconds = 18, commandDeadline = Date.now() + 25000) {
  const deadline = Math.min(commandDeadline, Date.now() + seconds * 1000);
  let previous = null;
  while (Date.now() < deadline) {
    await sleep(500);
    const current = await page(tabId, action, args, commandDeadline);
    if (accept(current, previous)) return current;
    previous = current;
  }
  // Whitelist only operational state: no search terms, titles, URLs or credentials.
  const evidence = previous ? Object.fromEntries([
    'document_focused', 'visibility', 'search_page', 'query_matches',
    'results_fresh', 'ready', 'title_matches', 'paused', 'player_state', 'position_seconds',
  ].filter(key => previous[key] !== undefined).map(key => [key, previous[key]])) : {};
  if (action === 'search_read') evidence.candidate_count = previous?.candidates?.length ?? 0;
  throw Error(`未能确认音乐操作结果；phase=${action} state=${JSON.stringify(evidence)}；请先查询状态，不要直接重复操作`);
}
async function execute(config, action, args = {}) {
  if (!actions.has(action)) throw Error('不支持的音乐操作');
  if (!Number.isInteger(config?.tabId)) throw Error('请先在扩展中绑定 YouTube Music 标签页');
  const deadline = config.deadline || Date.now() + 25000;
  const invoke = (operation, data = {}) => page(config.tabId, operation, data, deadline);
  const verify = (operation, data, accept, seconds = 18) => until(config.tabId, operation, data, accept, seconds, deadline);
  if (action === 'status') return invoke('status');
  if (action === 'search') {
    const query = String(args.query || '').trim();
    if (!query || Array.from(query).length > 200) throw Error('搜索词不能为空且最多 200 字');
    await chrome.storage.local.remove('candidates');
    const search_token = crypto.randomUUID();
    await invoke('search_start', { query, search_token });
    const result = await verify('search_read', { query, search_token }, searchConfirmed, 18);
    await chrome.storage.local.set({ candidates: result.candidates, candidateTab: config.tabId, candidateQuery: query });
    return { query, candidates: result.candidates };
  }
  const before = await invoke('status');
  if (action === 'play' || action === 'queue') {
    const id = args.video_id;
    if (!/^[A-Za-z0-9_-]{11}$/.test(id || '')) throw Error('需要搜索结果中的歌曲 ID');
    const stored = await chrome.storage.local.get(['candidates', 'candidateTab', 'candidateQuery']);
    if (stored.candidateTab !== config.tabId || !stored.candidates?.some(item => item.video_id === id)) throw Error('请先搜索，再使用搜索结果中的歌曲 ID');
    const selected = { ...args, search_query: stored.candidateQuery };
    if (action === 'queue') {
      const opened = await invoke('queue_open', selected);
      await sleep(350);
      await invoke('queue_click', selected);
      const result = await verify('queue_read', args, state => state.count > opened.before_count, 8);
      return { queued: true, video_id: id, playing: result };
    }
    await invoke('play', selected);
    const result = await verify('status', {}, (state, previous) =>
      playbackConfirmed(state, previous, id));
    return { ...result, playback_confirmed: true };
  }
  await invoke(action);
  const result = await verify('status', {}, (state, previous) => {
    if (action === 'pause') return state.paused === true && state.player_state === 2;
    if (action === 'resume') return playbackConfirmed(state, previous, before.video_id);
    return state.video_id !== before.video_id && playbackConfirmed(state, previous, state.video_id);
  });
  return { ...result, ...(action === 'pause' ? { pause_confirmed: true } : { playback_confirmed: true }) };
}
async function run(config, action, args) {
  if (busy) throw Error('音乐操作进行中，请等待完成');
  busy = true;
  try {
    await note('开始', { action, extension_version: extensionVersion });
    const result = await execute(config, action, args);
    await note('完成', { action, result });
    return result;
  } catch (error) {
    const message = `扩展 ${extensionVersion} / ${action}：${errorText(error)}`;
    await note('失败', { action, error: message });
    throw Error(message);
  } finally { busy = false; }
}
async function poll() {
  if (loopRunning) return;
  loopRunning = true;
  clearTimeout(timer);
  let nextMs = 1000;
  try {
    const config = await settings();
    if (!config?.enabled || !config.token || !config.tabId) { await connection('未连接：请配对并绑定音乐标签页'); return; }
    if (busy) return;
    const bound = await chrome.tabs.get(config.tabId).catch(() => null);
    if (!bound?.url?.startsWith('https://music.youtube.com/')) throw Error('音乐标签页已关闭或离开，请重新绑定');
    const response = await bridge(config, '/poll');
    await connection('已连接 Linger');
    const command = response.command;
    if (!command) return;
    if (typeof command.id !== 'string' && typeof command.id !== 'number') throw Error('命令没有有效 ID');
    // Persist receipt before side effects. An interrupted/re-delivered mutation is
    // never executed twice; its outcome is reported as unknown instead.
    const previous = (await chrome.storage.session.get('lastCommand')).lastCommand;
    let outcome;
    if (previous?.id === command.id && previous?.token === config.token) {
      outcome = previous.outcome || { id: command.id, error: '上一条操作被中断，结果未知；请先查询状态' };
    } else {
      await chrome.storage.session.set({ lastCommand: { id: command.id, token: config.token } });
      try { outcome = { id: command.id, result: await run({ ...config, deadline: Number(command.expires_at_ms) || Date.now() + 25000 }, command.action, command.args) }; }
      catch (error) { outcome = { id: command.id, error: errorText(error) }; }
      await chrome.storage.session.set({ lastCommand: { id: command.id, token: config.token, outcome } });
    }
    await bridge(config, '/result', outcome);
  } catch (error) {
    await connection(errorText(error));
    nextMs = 3000;
  } finally {
    loopRunning = false;
    timer = setTimeout(poll, nextMs);
  }
}
chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.id !== chrome.runtime.id || sender.url !== chrome.runtime.getURL('panel.html')) return;
  (async () => {
    if (message.type === 'save') {
      if (busy) throw Error('音乐操作进行中，暂时不能重新绑定');
      const { token, port, tabId, enabled } = message.settings;
      if (!/^[a-zA-Z0-9_-]{32,256}$/.test(token)) throw Error('请填写 Linger 提供的完整配对码');
      if (!Number.isInteger(port) || port < 1024 || port > 65535) throw Error('端口不正确');
      const tab = await chrome.tabs.get(tabId);
      if (!tab.url?.startsWith('https://music.youtube.com/')) throw Error('请选择 YouTube Music 标签页');
      await chrome.storage.local.set({ settings: { token, port, tabId, enabled: !!enabled } });
      await chrome.storage.local.remove(['candidates', 'candidateTab', 'candidateQuery']);
      void poll();
      return { saved: true };
    }
    if (message.type === 'disconnect') {
      if (busy) throw Error('音乐操作进行中，请完成后断开');
      await chrome.storage.local.remove('settings');
      await connection('已断开');
      return { disconnected: true };
    }
    if (message.type === 'test') return run(await settings(), message.action, message.args);
    throw Error('未知请求');
  })().then(result => respond({ result }), error => respond({ error: errorText(error) }));
  return true;
});
async function wake() {
  await chrome.storage.local.setAccessLevel({ accessLevel: 'TRUSTED_CONTEXTS' });
  if (!(await chrome.alarms.get('linger-music'))) await chrome.alarms.create('linger-music', { periodInMinutes: 0.5 });
  void poll();
}
chrome.alarms.onAlarm.addListener(alarm => { if (alarm.name === 'linger-music') void poll(); });
chrome.runtime.onStartup.addListener(wake);
chrome.runtime.onInstalled.addListener(wake);
void wake();
