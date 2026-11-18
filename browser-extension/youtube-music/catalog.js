// Injected as a self-contained function into the bound music page. The public
// catalogue accepts same-origin requests and does not require account cookies.
export async function catalogSearch(query, deadline) {
  try {
    if (location.origin !== 'https://music.youtube.com') throw Error('绑定页面已离开音乐站点');
    const version = window.ytcfg?.get?.('INNERTUBE_CLIENT_VERSION') || '1.20261117.01.00';
    const remaining = deadline - Date.now();
    if (remaining <= 0) throw Error('音乐命令已过期');
    const response = await fetch('https://music.youtube.com/youtubei/v1/search?prettyPrint=false', {
      method: 'POST', credentials: 'omit', redirect: 'error',
      headers: { 'Content-Type': 'application/json' },
      signal: AbortSignal.timeout(Math.min(6000, remaining)),
      body: JSON.stringify({ context: { client: { clientName: 'WEB_REMIX', clientVersion: version } },
        query, params: 'EgWKAQIIAWoKEAkQBRAKEAMQBA%3D%3D' }),
    });
    if (!response.ok) throw Error(`YouTube Music 搜索 HTTP ${response.status}`);
    const data = await response.json();
    const tabs = data.contents?.tabbedSearchResultsRenderer?.tabs || [];
    const tab = tabs.find(t => t.tabRenderer?.selected) || tabs[0];
    const shelves = tab?.tabRenderer?.content?.sectionListRenderer?.contents || [];
    const seen = new Set();
    const candidates = [];
    for (const shelf of shelves) {
      for (const item of shelf.musicShelfRenderer?.contents || []) {
        const row = item.musicResponsiveListItemRenderer;
        const id = row?.playlistItemData?.videoId || row?.overlay?.musicItemThumbnailOverlayRenderer
          ?.content?.musicPlayButtonRenderer?.playNavigationEndpoint?.watchEndpoint?.videoId;
        const columns = row?.flexColumns?.map(column => (column.musicResponsiveListItemFlexColumnRenderer
          ?.text?.runs || []).map(run => run.text).join('').trim()) || [];
        if (!/^[A-Za-z0-9_-]{11}$/.test(id || '') || !columns[0] || seen.has(id)) continue;
        seen.add(id);
        const detail = columns.slice(1).join(' • ').slice(0, 180);
        candidates.push({ video_id: id, title: columns[0].slice(0, 180), detail,
          duration: detail.match(/\b\d{1,2}:\d{2}(?::\d{2})?\b/)?.[0] || null });
        if (candidates.length === 8) return { candidates };
      }
    }
    if (!candidates.length) throw Error('公开目录未返回可识别的歌曲');
    return { candidates };
  } catch (error) {
    return { error: String(error?.message || error).slice(0, 180) };
  }
}
