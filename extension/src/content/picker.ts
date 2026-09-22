import { errorMessage, hostname } from '../shared/types';
import { pickerMatches, selectorFor } from './picker-selector';

let stop: (() => void) | undefined;
let interceptMouse: ((event: MouseEvent) => void) | undefined;
let interceptKey: ((event: KeyboardEvent) => void) | undefined;
// Register at document_start, before page handlers. Starting the picker later
// must not let an existing capture listener activate the element being picked.
for (const type of ['pointerdown', 'pointerup', 'mousedown', 'mouseup', 'click', 'auxclick'] as const) window.addEventListener(type, event => interceptMouse?.(event), true);
for (const type of ['keydown', 'keyup'] as const) window.addEventListener(type, event => interceptKey?.(event), true);
export function startPicker(token: string, refresh: () => Promise<void>): void {
  stop?.();
  const site = hostname(location.href);
  if (!site || !document.documentElement) throw new Error('Unsupported page.');
  const previousFocus = document.activeElement;
  const host = document.createElement('div');
  host.setAttribute('data-naab-picker', '');
  host.style.cssText = 'all:initial!important;position:fixed!important;inset:0!important;z-index:2147483647!important;pointer-events:auto!important;display:block!important';
  const root = host.attachShadow({ mode: 'closed' });
  const sheet = document.createElement('style');
  sheet.textContent = `
    :host{color-scheme:light}*{box-sizing:border-box}
    .outline{position:fixed;border:3px solid #087f8c;background:#087f8c22;pointer-events:none}
    .panel{position:fixed;bottom:16px;right:16px;width:min(420px,calc(100vw - 32px));max-height:calc(100vh - 32px);overflow:auto;padding:18px;border:2px solid #087f8c;border-radius:12px;background:#fff;color:#17262e;box-shadow:0 4px 28px #0005;pointer-events:auto;font:14px/1.5 system-ui,sans-serif}
    h2{font-size:18px;margin:0 0 8px}p{margin:8px 0}code{display:block;white-space:pre-wrap;overflow-wrap:anywhere;padding:8px;background:#edf3f5;font:12px/1.5 monospace}
    .buttons{display:flex;flex-wrap:wrap;gap:8px;margin-top:12px}button{font:inherit;padding:7px 11px;background:#f2f5f7;color:#17262e;border:1px solid #8b9ca6;border-radius:6px;cursor:pointer}button.primary{background:#087f8c;color:white;border-color:#087f8c}button:focus-visible{outline:3px solid #dc8300;outline-offset:2px}button:disabled{opacity:.5;cursor:default}.note{font-size:12px;color:#465a65}.error{color:#a01818}[hidden]{display:none!important}
  `;
  const outline = document.createElement('div'); outline.className = 'outline'; outline.hidden = true;
  const panel = document.createElement('section'); panel.className = 'panel'; panel.setAttribute('role', 'dialog'); panel.setAttribute('aria-label', 'NAAB element picker');
  const title = document.createElement('h2'); title.textContent = 'NAAB · Block something';
  const message = document.createElement('p'); message.setAttribute('role', 'status'); message.setAttribute('aria-live', 'polite');
  const code = document.createElement('code'); code.hidden = true;
  const note = document.createElement('p'); note.className = 'note'; note.textContent = 'Hides matching elements on this site and its subdomains. Class names can change. This does not stop downloads. Escape closes the picker.';
  const buttons = document.createElement('div'); buttons.className = 'buttons';
  const actions = new Map<HTMLButtonElement, () => void>();
  const button = (label: string, action: () => void, primary = false) => {
    const node = document.createElement('button'); node.type = 'button'; node.textContent = label; node.className = primary ? 'primary' : '';
    actions.set(node, action); buttons.append(node); return node;
  };
  let selected: Element | null = null;
  let selector: string | null = null;
  let hovered: Element | null = null;
  let previewStyle: HTMLStyleElement | undefined;
  let busy = false;
  let saved = false;
  let added = false;
  let closed = false;
  let frame = 0;
  const preview = button('Preview', () => { if (previewStyle) { clearPreview(); render(); } else showPreview(); }, true);
  const parent = button('Select parent', () => { if (selected?.parentElement) choose(selected.parentElement); });
  const again = button('Pick another', () => { clearPreview(); selected = selector = null; render(); });
  const save = button('Save rule', () => void persist(), true);
  const undo = button('Undo saved rule', () => void revert());
  const close = button('Cancel', () => finish());
  panel.append(title, message, code, note, buttons); root.append(sheet, outline, panel); document.documentElement.append(host);

  function clearPreview(): void { previewStyle?.remove(); previewStyle = undefined; }
  function matches(): Element[] { return selector ? pickerMatches(document, selector) : []; }
  function render(error?: string): void {
    message.className = error ? 'error' : '';
    const count = matches().length;
    message.textContent = error ?? (saved ? (added ? 'Rule saved. It will also apply after reloading this site.' : 'This rule is already in your local filters.') : !selected ? 'Point at an element and click to select it.' : !selector ? 'No supported selector for this element. Try its parent or pick another element.' : previewStyle ? `Preview: hiding ${count} matching element${count === 1 ? '' : 's'}. Save to keep this change.` : `${count} element${count === 1 ? '' : 's'} match. Preview before saving.`);
    code.hidden = !selector; code.textContent = selector ? `${site}##${selector}` : '';
    preview.hidden = saved || !selector; preview.textContent = previewStyle ? 'Restore preview' : 'Preview';
    parent.hidden = saved || !selected || selected.parentElement === document.body || selected.parentElement === document.documentElement;
    again.hidden = saved || !selected;
    save.hidden = saved || !selector; save.disabled = busy || !previewStyle;
    undo.hidden = !saved || !added;
    close.textContent = saved ? 'Done' : 'Cancel';
    for (const item of [preview, parent, again, undo, close]) item.disabled = busy;
    if (busy) message.textContent = 'Saving changes…';
    outline.hidden = !!previewStyle || saved || !hovered;
  }
  function choose(element: Element): void {
    clearPreview(); selected = element; hovered = element; selector = selectorFor(element); draw(); render();
    (selector ? preview : parent.hidden ? again : parent).focus();
  }
  function showPreview(): void {
    if (!selector || !selected?.isConnected || !matches().includes(selected)) { render('The page changed. Pick the element again.'); return; }
    previewStyle = document.createElement('style'); previewStyle.textContent = `${selector}{display:none!important}`;
    (document.head ?? document.documentElement).append(previewStyle); outline.hidden = true;
    render();
  }
  async function request(type: string, extra = {}): Promise<any> {
    const reply = await chrome.runtime.sendMessage({ type, token, ...extra });
    if (!reply?.ok) throw new Error(reply?.error ?? 'The extension did not respond. Close and reopen the picker.');
    return reply.payload;
  }
  async function persist(): Promise<void> {
    if (!selector || !previewStyle) return;
    if (!selected?.isConnected || !matches().includes(selected)) { clearPreview(); render('The page changed. Pick the element again.'); return; }
    busy = true; render();
    try {
      const result = await request('picker.save', { selector }); saved = true; added = result.added;
      await refresh(); clearPreview();
      busy = false; render(); (added ? undo : close).focus();
    } catch (error) { busy = false; clearPreview(); render(errorMessage(error)); }
  }
  async function revert(): Promise<void> {
    busy = true; render();
    try { await request('picker.undo'); await refresh(); busy = false; finish(); }
    catch (error) { busy = false; render(errorMessage(error)); }
  }
  function draw(): void {
    frame = 0;
    if (!hovered?.isConnected || previewStyle || saved) { outline.hidden = true; return; }
    const rect = hovered.getBoundingClientRect(); outline.hidden = false;
    outline.style.cssText = `left:${rect.left}px;top:${rect.top}px;width:${rect.width}px;height:${rect.height}px`;
  }
  const schedule = () => { if (!frame) frame = requestAnimationFrame(draw); };
  function insidePanel(x: number, y: number): boolean {
    const rect = panel.getBoundingClientRect(); return x >= rect.left && x <= rect.right && y >= rect.top && y <= rect.bottom;
  }
  function underlying(x: number, y: number): Element | null {
    // The overlay catches frame clicks too. Hit testing returns the iframe
    // element, never an element in its inaccessible child document.
    host.style.setProperty('pointer-events', 'none', 'important');
    try { return document.elementFromPoint(x, y); }
    finally { host.style.setProperty('pointer-events', 'auto', 'important'); }
  }
  function move(event: MouseEvent): void {
    if (selected || busy || saved || insidePanel(event.clientX, event.clientY)) return;
    hovered = underlying(event.clientX, event.clientY); schedule();
  }
  function blockPageClick(event: MouseEvent): void {
    event.preventDefault(); event.stopImmediatePropagation();
    if (insidePanel(event.clientX, event.clientY)) {
      const hit = (event.detail === 0 && event.type === 'click' ? root.activeElement : root.elementFromPoint(event.clientX, event.clientY))?.closest('button');
      if (hit instanceof HTMLButtonElement && !hit.disabled && !hit.hidden && event.isTrusted) {
        if (event.type === 'pointerdown' || event.type === 'mousedown') hit.focus();
        if (event.type === 'click' && !busy) actions.get(hit)?.();
      }
      return;
    }
    if (event.type === 'click' && event.isTrusted && !selected && !busy && !saved) {
      const target = underlying(event.clientX, event.clientY); if (target) choose(target);
    }
  }
  function key(event: KeyboardEvent): void {
    if (!event.isTrusted) return;
    event.stopImmediatePropagation();
    if (event.type !== 'keydown') return;
    if (event.key === 'Enter' || event.key === ' ') {
      const focused = root.activeElement;
      if (focused instanceof HTMLButtonElement) { event.preventDefault(); if (!focused.disabled && !busy && !event.repeat) actions.get(focused)?.(); }
      return;
    }
    if (event.key === 'Escape') { event.preventDefault(); event.stopImmediatePropagation(); if (!busy) finish(); return; }
    // Keep keyboard focus in the small control panel while picking.
    if (event.key === 'Tab') {
      const visible = Array.from(buttons.querySelectorAll('button')).filter(item => !item.hidden && !item.disabled);
      if (!visible.length) return;
      const index = visible.indexOf(root.activeElement as HTMLButtonElement);
      const next = event.shiftKey ? (index <= 0 ? visible.length - 1 : index - 1) : (index + 1) % visible.length;
      event.preventDefault(); event.stopImmediatePropagation(); visible[next].focus();
    }
  }
  function cleanup(): void {
    if (closed) return; closed = true;
    clearPreview(); host.remove(); cancelAnimationFrame(frame);
    document.removeEventListener('mousemove', move, true);
    if (interceptMouse === blockPageClick) interceptMouse = undefined;
    if (interceptKey === key) interceptKey = undefined;
    window.removeEventListener('scroll', schedule, true); window.removeEventListener('resize', schedule);
    window.removeEventListener('pagehide', cleanup);
    if (previousFocus instanceof HTMLElement && previousFocus.isConnected) previousFocus.focus({ preventScroll: true });
    if (stop === cleanup) stop = undefined;
  }
  function finish(): void { cleanup(); void request('picker.cancel').catch(() => {}); }
  stop = cleanup;
  document.addEventListener('mousemove', move, true);
  interceptMouse = blockPageClick;
  interceptKey = key;
  window.addEventListener('scroll', schedule, true); window.addEventListener('resize', schedule);
  window.addEventListener('pagehide', cleanup, { once: true });
  render(); close.focus();
}
