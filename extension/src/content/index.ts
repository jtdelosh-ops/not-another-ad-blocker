import { MAX_SELECTORS, isSafeSelector } from '../shared/types';
import { startPicker } from './picker';
let style: HTMLStyleElement | undefined;
let revision = 0;
async function refresh(): Promise<void> {
  const current = ++revision;
  try {
    const response = await chrome.runtime.sendMessage({ type: 'cosmetics.get' });
    if (current !== revision || !response?.ok || !Array.isArray(response.payload?.selectors)) return;
    const selectors = response.payload.selectors;
    if (selectors.length > MAX_SELECTORS || !selectors.every(isSafeSelector)) return;
    style?.remove();
    style = undefined;
    if (!selectors.length) return;
    // Only validated compounds or single direct-child :has() selectors enter
    // this stylesheet. No declarations, URLs, attributes or scriptlets.
    const next = document.createElement('style');
    next.textContent = [...new Set<string>(selectors)].map(selector => `${selector}{display:none!important}`).join('\n');
    (document.head ?? document.documentElement)?.append(next);
    style = next;
  } catch { /* Existing CSS stays active if the service worker is unavailable. */ }
}
chrome.runtime.onMessage.addListener((message, sender, respond) => {
  if (sender.id === chrome.runtime.id && message?.type === 'cosmetics.refresh') void refresh();
  if (sender.id === chrome.runtime.id && message?.type === 'picker.start' && typeof message.token === 'string') {
    try { startPicker(message.token, refresh); respond({ ok: true }); }
    catch { respond({ ok: false }); }
  }
});
if (document.documentElement) void refresh();
else document.addEventListener('DOMContentLoaded', () => void refresh(), { once: true });
