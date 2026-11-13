const $ = id => document.getElementById(id);
let initialized = false;
async function refresh() {
  const state = await chrome.storage.local.get(['settings', 'connection', 'logs', 'candidates']);
  $('connection').textContent = state.connection || '尚未配对';
  $('logs').textContent = (state.logs || []).join('\n');
  if (!initialized) {
    const tabs = await chrome.tabs.query({ url: 'https://music.youtube.com/*' });
    for (const tab of tabs) $('tab').add(new Option(`${tab.title || 'YouTube Music'}（${tab.id}）`, String(tab.id)));
    if (!tabs.length) $('tab').add(new Option('请先打开 YouTube Music，然后重新打开本面板', ''));
    if (state.settings) {
      $('token').value = state.settings.token || '';
      $('port').value = state.settings.port || 47831;
      $('tab').value = String(state.settings.tabId);
    }
    initialized = true;
  }
  const selected = $('candidate').value;
  $('candidate').replaceChildren();
  for (const item of state.candidates || []) $('candidate').add(new Option(`${item.title} — ${item.detail}`, item.video_id));
  if (selected && [...$('candidate').options].some(option => option.value === selected)) $('candidate').value = selected;
}
async function request(message) {
  const buttons = [...document.querySelectorAll('button')];
  buttons.forEach(button => { button.disabled = true; });
  $('feedback').textContent = '处理中…可以关闭面板，操作会继续。';
  try {
    const response = await chrome.runtime.sendMessage(message);
    if (response?.error) throw Error(response.error);
    const result = response?.result;
    $('feedback').textContent = result?.saved ? '已保存，正在连接 Linger。' : result?.disconnected ? '已断开。' :
      result?.candidates ? `找到 ${result.candidates.length} 首歌曲。` :
      result?.queued ? '已加入队列。' : result?.title ? `${result.paused ? '已暂停' : '播放中'}：${result.title}\n${result.artist || ''}` : '操作完成。';
    await refresh();
  } catch (error) { $('feedback').textContent = error.message || String(error); }
  finally { buttons.forEach(button => { button.disabled = false; }); }
}
$('save').onclick = () => request({ type: 'save', settings: {
  token: $('token').value.trim(), port: Number($('port').value), tabId: Number($('tab').value), enabled: true,
} });
$('disconnect').onclick = () => request({ type: 'disconnect' });
$('search').onclick = () => request({ type: 'test', action: 'search', args: { query: $('query').value.trim() } });
$('query').onkeydown = event => { if (event.key === 'Enter' && !$('search').disabled) $('search').click(); };
for (const action of ['status', 'pause', 'resume', 'next']) $(action).onclick = () => request({ type: 'test', action });
for (const action of ['play', 'queue']) $(action).onclick = () => request({ type: 'test', action, args: { video_id: $('candidate').value } });
chrome.storage.onChanged.addListener((_changes, area) => { if (area === 'local') void refresh(); });
void refresh();
