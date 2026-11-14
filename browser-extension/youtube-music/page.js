// Runs only in the user-bound YouTube Music tab, in MAIN world. No credentials,
// network calls, full navigations, focus changes or low-level loadVideoById calls.
export function pageAction(action, args) {
  if (Number(args.__deadline_ms) <= Date.now()) throw Error('音乐命令已过期，未执行');
  if (location.origin !== 'https://music.youtube.com') throw Error('绑定标签页已离开 YouTube Music');
  const player = document.getElementById('movie_player');
  const bar = document.querySelector('ytmusic-player-bar');
  const visible = element => {
    if (!element || !element.getClientRects().length) return false;
    for (let node = element; node; node = node.parentElement) {
      const style = getComputedStyle(node);
      if (node.hidden || node.getAttribute('aria-hidden') === 'true' || style.display === 'none' || style.visibility === 'hidden') return false;
    }
    return true;
  };
  const clean = (text, limit = 180) => String(text || '').trim().replace(/\s+/g, ' ').slice(0, limit);
  const idOf = element => {
    const endpoint = element?.playCommand?.watchEndpoint || element?.controllerProxy?.playCommand?.watchEndpoint;
    const data = element?.data;
    return endpoint?.videoId || data?.videoId || data?.playlistPanelVideoRenderer?.videoId || null;
  };
  const queueIds = () => [...document.querySelectorAll('ytmusic-player-queue-item')].slice(0, 250)
    .map(row => idOf(row) || idOf(row.querySelector('ytmusic-play-button-renderer'))).filter(Boolean);
  const status = () => {
    const video = document.querySelector('video');
    const data = typeof player?.getVideoData === 'function' ? player.getVideoData() : {};
    const title = clean(data?.title);
    const barTitle = clean(bar?.querySelector('.title')?.textContent);
    const state = typeof player?.getPlayerState === 'function' ? player.getPlayerState() : null;
    const videoId = data?.video_id || null;
    return {
      video_id: videoId, title, artist: clean(bar?.querySelector('.byline')?.textContent),
      bar_title: barTitle, paused: video?.paused ?? null, player_state: state,
      position_seconds: Number.isFinite(video?.currentTime) ? video.currentTime : null,
      duration_seconds: Number.isFinite(video?.duration) ? video.duration : null,
      ready: !!player && !!bar && !!video,
      title_matches: !!videoId && !!title && title === barTitle,
      queue_count: queueIds().length,
      document_focused: document.hasFocus(), visibility: document.visibilityState,
    };
  };
  if (action === 'status') return status();
  if (action === 'search_start') {
    const previousQuery = new URL(location.href).searchParams.get('q');
    // Keep one bounded snapshot across MAIN-world invocations. Song IDs alone
    // cannot prove freshness: different queries can legitimately return the same songs.
    const searchKey = Symbol.for('linger.music.search');
    const searchRows = [...document.querySelectorAll('ytmusic-search-page ytmusic-responsive-list-item-renderer')].slice(0, 100);
    const priorSearch = document[searchKey];
    document[searchKey] = {
      token: args.search_token, query: args.query, deadline: args.__deadline_ms,
      fresh: previousQuery === args.query && !(priorSearch?.query === args.query && !priorSearch.fresh),
      rows: searchRows.map(node => ({ node, data: node.data, controllerData: node.controllerProxy?.data,
        id: idOf(node.querySelector('ytmusic-play-button-renderer')), text: clean(node.textContent) })),
    };
    const input = document.querySelector('ytmusic-search-box input');
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')?.set;
    if (!input || !setter) throw Error('未找到站内搜索框；请在已登录的 YouTube Music 页面重试');
    setter.call(input, args.query);
    input.dispatchEvent(new Event('input', { bubbles: true, composed: true }));
    input.dispatchEvent(new Event('change', { bubbles: true, composed: true }));
    // The site's Polymer key binding is enter:keypress. Synthetic keydown does
    // not generate keypress automatically, unlike a real keyboard event.
    for (const type of ['keydown', 'keypress', 'keyup']) input.dispatchEvent(new KeyboardEvent(type, {
      key: 'Enter', code: 'Enter', keyCode: 13, charCode: type === 'keypress' ? 13 : 0, which: 13, bubbles: true, cancelable: true, composed: true,
    }));
    return { submitted: true, previous_query: previousQuery,
      document_focused: document.hasFocus(), visibility: document.visibilityState };
  }
  const rows = [...document.querySelectorAll('ytmusic-search-page ytmusic-responsive-list-item-renderer')].slice(0, 100);
  const rowId = row => idOf(row.querySelector('ytmusic-play-button-renderer')) || (() => {
    const href = row.querySelector('a[href*="watch?"]')?.getAttribute('href');
    return href ? new URL(href, location.href).searchParams.get('v') : null;
  })();
  if (action === 'search_read') {
    const url = new URL(location.href);
    const snapshot = document[Symbol.for('linger.music.search')];
    const tracked = args.search_token && snapshot?.token === args.search_token && snapshot.query === args.query && snapshot.deadline > Date.now();
    if (tracked && !snapshot.fresh && url.pathname === '/search' && url.searchParams.get('q') === args.query) {
      // Polymer may reuse a row node while replacing its renderer data. Check
      // both identities plus content, never assume a changed URL refreshed rows.
      snapshot.fresh = rows.some(node => visible(node) && !snapshot.rows.some(old =>
        old.node === node && old.data === node.data && old.controllerData === node.controllerProxy?.data &&
        old.id === idOf(node.querySelector('ytmusic-play-button-renderer')) && old.text === clean(node.textContent)));
      if (snapshot.fresh) snapshot.rows = [];
    }
    const diagnostics = {
      results_fresh: !!tracked && snapshot.fresh,
      document_focused: document.hasFocus(), visibility: document.visibilityState,
      search_page: url.pathname === '/search', query_matches: url.searchParams.get('q') === args.query,
    };
    if (!diagnostics.search_page || !diagnostics.query_matches) return { ready: false, candidates: [], ...diagnostics };
    const seen = new Set();
    const candidates = [];
    for (const row of rows) {
      const videoId = rowId(row);
      if (!visible(row) || !videoId || seen.has(videoId) || !row.querySelector('ytmusic-play-button-renderer')) continue;
      seen.add(videoId);
      const detail = clean(row.textContent);
      candidates.push({
        video_id: videoId, title: clean(row.querySelector('.title')?.textContent),
        detail, duration: detail.match(/\b\d{1,2}:\d{2}(?::\d{2})?\b/)?.[0] || null,
      });
      if (candidates.length === 8) break;
    }
    return { ready: candidates.length > 0, candidates, ...diagnostics };
  }
  if (['play', 'queue_open', 'queue_click'].includes(action)) {
    const url = new URL(location.href);
    if (url.pathname !== '/search' || (args.search_query && url.searchParams.get('q') !== args.search_query)) {
      throw Error('已离开对应搜索结果页，请重新搜索后播放或排队');
    }
  }
  if (action === 'play' || action === 'queue_open') {
    const row = rows.find(row => rowId(row) === args.video_id && visible(row));
    if (!row) throw Error('该歌曲已不在当前搜索结果中，请重新搜索后播放或排队');
    if (action === 'play') {
      const button = row.querySelector('ytmusic-play-button-renderer');
      if (!button) throw Error('没有找到歌曲的站内播放按钮');
      button.click();
      return { issued: true };
    }
    const menu = row.querySelector('ytmusic-menu-renderer button, ytmusic-menu-renderer tp-yt-paper-icon-button');
    if (!menu) throw Error('没有找到歌曲菜单，无法加入队列');
    const beforeCount = queueIds().filter(id => id === args.video_id).length;
    menu.click();
    return { issued: false, before_count: beforeCount };
  }
  if (action === 'queue_click') {
    const items = [...document.querySelectorAll('ytmusic-popup-container ytmusic-menu-popup-renderer ytmusic-menu-service-item-renderer')];
    const item = items.find(item => {
      const popup = item.closest('ytmusic-menu-popup-renderer');
      if (!popup?.closest('ytmusic-popup-container') || !visible(popup) || !visible(item)) return false;
      const data = item.data || item.controllerProxy?.data;
      const command = data?.serviceEndpoint?.queueAddEndpoint;
      return command?.queueTarget?.videoId === args.video_id && command.queueInsertPosition === 'INSERT_AT_END';
    });
    if (!item) throw Error('未找到可识别的「加入队列」菜单项，未更改队列');
    item.click();
    return { issued: true };
  }
  if (action === 'queue_read') return { count: queueIds().filter(id => id === args.video_id).length, ...status() };
  if (!player || !bar) throw Error('播放器尚未就绪，请先在 YouTube Music 手动播放一次');
  const current = status();
  if (action === 'pause' || action === 'resume') {
    if (!current.ready || typeof current.paused !== 'boolean') throw Error('播放状态尚未就绪，未切换播放/暂停');
    const paused = action === 'pause';
    if (current.paused === paused) return { issued: false, already_in_state: true };
    const button = bar.querySelector('#play-pause-button');
    if (!button) throw Error('未找到播放/暂停按钮');
    button.click();
    return { issued: true };
  }
  if (action === 'next') {
    const button = bar.querySelector('.next-button');
    if (!button || button.disabled || button.getAttribute('aria-disabled') === 'true') throw Error('下一首按钮不可用');
    button.click();
    return { issued: true };
  }
  throw Error('不支持的音乐操作');
}

// A localized/bilingual bottom-bar title can differ from getVideoData().title.
// Titles remain diagnostics; playback is proved by ID and advancing media time.
export function playbackConfirmed(state, previous, expectedId) {
  return !!expectedId && state.video_id === expectedId && state.paused === false && state.player_state === 1 &&
    previous?.video_id === expectedId && Number.isFinite(state.position_seconds) &&
    Number.isFinite(previous.position_seconds) && state.position_seconds > previous.position_seconds;
}

export function searchConfirmed(state, previous) {
  return !!(state.ready && state.results_fresh && previous?.ready && previous.results_fresh &&
    JSON.stringify(state.candidates) === JSON.stringify(previous.candidates));
}
