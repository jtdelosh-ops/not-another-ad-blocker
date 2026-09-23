import { disabledBy, hostname, isSafeSelector } from '../shared/types';
import type { Controller } from './controller';
import type { Sender } from './routes';

export interface PickerGrant { token: string; host: string; expires: number; selector?: string }
export interface PickerBackend {
  tab(id: number): Promise<{ url?: string; active?: boolean; incognito?: boolean }>;
  read(id: number): Promise<PickerGrant | undefined>;
  write(id: number, grant: PickerGrant): Promise<void>;
  remove(id: number): Promise<void>;
  start(id: number, token: string): Promise<unknown>;
}
export class Picker {
  private queue: Promise<unknown> = Promise.resolve();
  constructor(private controller: Controller, private backend: PickerBackend, private now = Date.now, private token = () => crypto.randomUUID()) {}
  private serial<T>(action: () => Promise<T>): Promise<T> {
    const job = this.queue.then(action); this.queue = job.catch(() => {}); return job;
  }
  start(tabId: unknown): Promise<void> {
    return this.serial(async () => {
      if (!Number.isSafeInteger(tabId) || (tabId as number) < 0) throw new Error('Invalid picker tab.');
      const id = tabId as number;
      const tab = await this.backend.tab(id);
      const host = hostname(tab.url);
      if (!host || !tab.active || tab.incognito) throw new Error('Open a normal HTTP(S) page to use the picker. Private tabs are not supported.');
      const config = await this.controller.snapshot();
      if (!config.enabled || disabledBy(host, config.disabledSites)) throw new Error('Enable protection for this site before using the picker.');
      const token = this.token();
      await this.backend.write(id, { token, host, expires: this.now() + 10 * 60_000 });
      try {
        const result = await this.backend.start(id, token) as { ok?: boolean } | undefined;
        if (!result?.ok) throw new Error('Picker could not start.');
      } catch {
        await this.backend.remove(id);
        throw new Error('Reload this page, then try the picker again. Browser-restricted pages cannot be picked.');
      }
    });
  }
  handle(input: Record<string, unknown>, sender: Sender): Promise<unknown> {
    return this.serial(async () => {
      const id = sender.tab?.id;
      const host = hostname(sender.url);
      if (id === undefined || sender.frameId !== 0 || sender.tab?.incognito || !host) throw new Error('Picker requests must come from the selected page.');
      const grant = await this.backend.read(id);
      if (!grant || input.token !== grant.token || grant.host !== host || grant.expires <= this.now()) throw new Error('Picker session expired. Close it and start again from NAAB.');
      if (input.type === 'picker.cancel') { await this.backend.remove(id); return null; }
      if (input.type === 'picker.undo') {
        if (!grant.selector) throw new Error('There is no picker rule to undo.');
        await this.controller.removePickerRule(host, grant.selector, grant.token);
        await this.backend.remove(id);
        return null;
      }
      if (input.type !== 'picker.save' || !isSafeSelector(input.selector) || !/[.#]/.test(input.selector) || (grant.selector && grant.selector !== input.selector)) throw new Error('Invalid picker rule.');
      // Persist the permission before changing filters. Retried saves are
      // idempotent; an interrupted reply can still be undone after worker sleep.
      await this.backend.write(id, { ...grant, selector: input.selector });
      try { return await this.controller.addPickerRule(host, input.selector, grant.token); }
      catch (error) { await this.backend.write(id, grant).catch(() => {}); throw error; }
    });
  }
}
