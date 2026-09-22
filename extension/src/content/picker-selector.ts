import { isSafeSelector } from '../shared/types';

export function pickerMatches(document: Document, selector: string): Element[] {
  if (!isSafeSelector(selector)) return [];
  const matches = Array.from(document.querySelectorAll(selector));
  // Never offer a rule that hides the page root or the picker's own interface.
  if (matches.some(el => el === document.documentElement || el === document.body || el === document.head || el.hasAttribute('data-naab-picker'))) return [];
  return matches;
}

export function selectorFor(element: Element): string | null {
  const document = element.ownerDocument;
  if (element.getRootNode() !== document || [document.documentElement, document.body, document.head].includes(element as HTMLElement)) return null;
  const candidates = new Set<string>();
  const add = (value: string) => { if (isSafeSelector(value)) candidates.add(value); };
  const tag = element.localName;
  if (element.id) add(`#${element.id}`);
  const classes = Array.from(element.classList).slice(0, 8);
  for (const name of classes) { add(`.${name}`); add(`${tag}.${name}`); }
  for (let i = 0; i < classes.length; i++) for (let j = i + 1; j < classes.length; j++) add(`${tag}.${classes[i]}.${classes[j]}`);
  // A stable child can identify a wrapper whose own class changes on reload.
  for (const child of Array.from(element.children).slice(0, 12)) {
    if (child.id) add(`${tag}:has(>#${child.id})`);
    for (const name of Array.from(child.classList).slice(0, 4)) add(`${tag}:has(>.${name})`);
  }
  const ranked = [...candidates].map(selector => {
    const matches = pickerMatches(document, selector);
    const unstable = /\d{5}|[a-f0-9]{12}/i.test(selector) ? 1000 : 0;
    return { selector, matches, score: unstable + (matches.length > 1 ? 100 : 0) + selector.length };
  }).filter(item => item.matches.includes(element) && item.matches.length <= 100);
  ranked.sort((a, b) => a.score - b.score);
  return ranked[0]?.selector ?? null;
}
