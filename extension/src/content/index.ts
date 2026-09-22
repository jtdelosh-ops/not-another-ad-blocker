import { MAX_SELECTORS, isSafeSelector } from '../shared/types';
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
    // Only validated compound selectors enter the stylesheet. No rule text,
    // declarations, URLs, attributes, or scriptlets can reach this sink.
    const next = document.createElement('style');
    next.textContent = [...new Set<string>(selectors)].map(selector => `${selector}{display:none!important}`).join('\n');
    (document.head ?? document.documentElement)?.append(next);
    style = next;
  } catch { /* Existing CSS stays active if the service worker is unavailable. */ }
}
chrome.runtime.onMessage.addListener((message, sender) => {
  if (sender.id === chrome.runtime.id && message?.type === 'cosmetics.refresh') void refresh();
});
if (document.documentElement) void refresh();
else document.addEventListener('DOMContentLoaded', () => void refresh(), { once: true });
