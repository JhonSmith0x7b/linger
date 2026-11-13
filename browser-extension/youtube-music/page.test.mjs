import test from 'node:test';
import assert from 'node:assert/strict';
import { pageAction } from './page.js';
const visibleElement = () => ({ getClientRects: () => [{}], getAttribute: () => null, parentElement: null });
const deadline = () => ({ __deadline_ms: Date.now() + 5000 });
function fixture({ rows = [], menuItems = [], queue = [], title = 'Song', barTitle = title } = {}) {
  const player = { getVideoData: () => ({ video_id: 'abcdefghijk', title }), getPlayerState: () => 1 };
  const video = { paused: false, currentTime: 15, duration: 120 };
  const bar = { querySelector: selector => selector === '.title' ? { textContent: barTitle } : { textContent: 'Singer' } };
  globalThis.getComputedStyle = () => ({ display: 'block', visibility: 'visible' });
  globalThis.location = { origin: 'https://music.youtube.com', href: 'https://music.youtube.com/search?q=song' };
  globalThis.document = {
    getElementById: () => player,
    querySelector: selector => selector === 'video' ? video : selector === 'ytmusic-player-bar' ? bar : null,
    querySelectorAll: selector => selector.includes('responsive-list-item') ? rows : selector.includes('menu-service-item') ? menuItems : selector.includes('player-queue-item') ? queue : [],
    hasFocus: () => false, visibilityState: 'hidden',
  };
}
function row(id, clicked = () => {}) {
  const button = { playCommand: { watchEndpoint: { videoId: id } }, click: clicked };
  return { ...visibleElement(), textContent: 'Song Singer 3:42', querySelector: selector => selector === 'ytmusic-play-button-renderer' ? button : selector === '.title' ? { textContent: 'Song' } : null };
}
test('search returns bounded de-duplicated IDs without exposing full commands', () => {
  fixture({ rows: [row('abcdefghijk'), row('abcdefghijk'), ...Array.from({ length: 12 }, (_, i) => row(String(i).padStart(11, '0')))] });
  const result = pageAction('search_read', { ...deadline(), query: 'song' });
  assert.equal(result.candidates.length, 8);
  assert.equal(result.candidates[0].video_id, 'abcdefghijk');
  assert.equal(result.candidates[0].duration, '3:42');
  assert.equal(JSON.stringify(result).includes('watchEndpoint'), false);
});
test('play uses the matching site button and refuses stale result IDs', () => {
  let clicks = 0;
  fixture({ rows: [row('abcdefghijk', () => clicks++)] });
  assert.deepEqual(pageAction('play', { ...deadline(), video_id: 'abcdefghijk' }), { issued: true });
  assert.equal(clicks, 1);
  assert.throws(() => pageAction('play', { ...deadline(), video_id: '12345678901' }), /重新搜索/);
  assert.equal(clicks, 1);
});
test('expired commands never invoke a player action', () => {
  let clicks = 0;
  fixture({ rows: [row('abcdefghijk', () => clicks++)] });
  assert.throws(() => pageAction('play', { __deadline_ms: 1, video_id: 'abcdefghijk' }), /过期/);
  assert.equal(clicks, 0);
});
test('status detects underlying player and visible bar disagreement', () => {
  fixture({ barTitle: 'Old song' });
  assert.equal(pageAction('status', deadline()).synchronized, false);
});
test('queue click targets append for the selected ID, not next or another song', () => {
  const clicked = [];
  const item = (id, position, label) => ({ ...visibleElement(), closest: () => ({ ...visibleElement(), closest: () => ({}) }), data: { serviceEndpoint: { queueAddEndpoint: {
    queueTarget: { videoId: id }, queueInsertPosition: position,
  } } }, textContent: label, click: () => clicked.push(id + position) });
  fixture({ menuItems: [item('abcdefghijk', 'INSERT_AFTER_CURRENT_VIDEO', 'Play next'), item('12345678901', 'INSERT_AT_END', 'Add to queue'), item('abcdefghijk', 'INSERT_AT_END', 'Add to queue')] });
  pageAction('queue_click', { ...deadline(), video_id: 'abcdefghijk' });
  assert.deepEqual(clicked, ['abcdefghijkINSERT_AT_END']);
});
test('queue evidence is linked to the requested ID', () => {
  fixture({ queue: [{ data: { videoId: 'abcdefghijk' } }, { data: { videoId: '12345678901' } }] });
  assert.equal(pageAction('queue_read', { ...deadline(), video_id: 'abcdefghijk' }).count, 1);
});

test('hidden cached search rows are not played', () => {
  let clicks = 0;
  const cached = row('abcdefghijk', () => clicks++);
  cached.getClientRects = () => [];
  fixture({ rows: [cached] });
  assert.throws(() => pageAction('play', { ...deadline(), video_id: 'abcdefghijk' }), /重新搜索/);
  assert.equal(clicks, 0);
});
test('cached search rows cannot be used off the matching search page', () => {
  let clicks = 0;
  fixture({ rows: [row('abcdefghijk', () => clicks++)] });
  globalThis.location.href = 'https://music.youtube.com/watch?v=abcdefghijk';
  assert.throws(() => pageAction('play', { ...deadline(), video_id: 'abcdefghijk' }), /重新搜索/);
  globalThis.location.href = 'https://music.youtube.com/search?q=different';
  assert.throws(() => pageAction('queue_open', { ...deadline(), video_id: 'abcdefghijk', search_query: 'song' }), /重新搜索/);
  assert.equal(clicks, 0);
});
test('queue ignores hidden menu entries and refuses label-only fallback', () => {
  let clicks = 0;
  const base = { ...visibleElement(), textContent: 'Add to queue', closest: () => ({ ...visibleElement(), closest: () => ({}) }), click: () => clicks++ };
  const hidden = { ...base, getClientRects: () => [], data: { serviceEndpoint: { queueAddEndpoint: { queueTarget: { videoId: 'abcdefghijk' }, queueInsertPosition: 'INSERT_AT_END' } } } };
  fixture({ menuItems: [hidden, base] });
  assert.throws(() => pageAction('queue_click', { ...deadline(), video_id: 'abcdefghijk' }), /未找到/);
  assert.equal(clicks, 0);
});

test('pause does not toggle when current media state is unknown', () => {
  fixture();
  const originalQuery = document.querySelector;
  document.querySelector = selector => selector === 'video' ? null : originalQuery(selector);
  assert.throws(() => pageAction('pause', deadline()), /尚未就绪/);
  assert.throws(() => pageAction('resume', deadline()), /尚未就绪/);
});

test('search submits through enter:keypress instead of only updating input', () => {
  fixture();
  const events = [];
  let submitted = null;
  globalThis.HTMLInputElement = class {
    set value(value) { this.text = value; }
    dispatchEvent(event) {
      events.push(event.type);
      if (event.type === 'keypress' && event.key === 'Enter' && event.charCode === 13) submitted = this.text;
      return true;
    }
  };
  globalThis.KeyboardEvent = class extends Event {
    constructor(type, init) { super(type, init); this.key = init.key; this.charCode = init.charCode; }
  };
  const input = new HTMLInputElement();
  const original = document.querySelector;
  document.querySelector = selector => selector === 'ytmusic-search-box input' ? input : original(selector);
  pageAction('search_start', { ...deadline(), query: '周杰伦 东风破' });
  assert.equal(submitted, '周杰伦 东风破');
  assert.deepEqual(events, ['input', 'change', 'keydown', 'keypress', 'keyup']);
});
