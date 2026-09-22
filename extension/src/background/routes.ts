import { hostname } from '../shared/types';
import type { Controller } from './controller';
import { configView } from './controller';
import { validateSubscriptionIds } from '../shared/native-client';
import type { ActivityAccess } from '../shared/activity';
import type { Picker } from './picker';

export interface Sender { id?: string; url?: string; tab?: { id?: number; incognito?: boolean }; frameId?: number }
export function createRouter(controller: Controller, extensionId: string, extensionURL: string, activity?: ActivityAccess, picker?: Picker) {
  return async (message: unknown, sender: Sender): Promise<unknown> => {
    if (sender.id !== extensionId || !message || typeof message !== 'object') throw new Error('Unauthorized message.');
    const input = message as Record<string, unknown>;
    if (['picker.save', 'picker.undo', 'picker.cancel'].includes(input.type as string)) {
      if (!picker) throw new Error('Picker unavailable.');
      return picker.handle(input, sender);
    }
    if (input.type === 'cosmetics.get') {
      const host = hostname(sender.url);
      if (!sender.tab || sender.frameId !== 0 || !host) throw new Error('Cosmetic configuration is only available to an HTTP(S) top frame.');
      return controller.cosmetics(host);
    }
    let page = '';
    try { const url = new URL(sender.url ?? ''); url.hash = ''; page = url.href; } catch { /* Invalid senders remain untrusted. */ }
    const interfacePages = ['popup.html', 'options.html', 'activity.html'].map(name => new URL(name, extensionURL).href);
    const trustedPage = !sender.tab && interfacePages.includes(page);
    // Options opened in a tab legitimately have sender.tab; trust the precise
    // packaged URL, extension ID, and top frame, never a web-provided origin.
    const trustedTab = sender.frameId === 0 && ['options.html', 'activity.html'].map(name => new URL(name, extensionURL).href).includes(page);
    if (!trustedPage && !trustedTab) throw new Error('This operation is only available from the extension interface.');
    switch (input.type) {
      case 'picker.start':
        if (!picker) throw new Error('Picker unavailable.');
        return picker.start(input.tabId);
      case 'activity.get':
        if (input.tabId !== null && (!Number.isSafeInteger(input.tabId) || (input.tabId as number) < 0 || (input.tabId as number) > 2_147_483_647)) throw new Error('Invalid activity tab.');
        if (!activity) throw new Error('Activity is unavailable.');
        return activity.get(input.tabId as number | null);
      case 'activity.clear':
        if (!activity) throw new Error('Activity is unavailable.');
        await activity.clear();
        return null;
      case 'config.get': return controller.view();
      case 'subscriptions.progress': return controller.subscriptionProgress();
      case 'subscriptions.refresh': return configView(await controller.refreshSubscriptions(validateSubscriptionIds(input.ids)));
      case 'subscriptions.remove': return configView(await controller.removeSubscriptions());
      case 'status.get': return controller.native.status();
      case 'config.import':
        if (typeof input.text !== 'string' || typeof input.source !== 'string') throw new Error('Invalid list import.');
        return configView(await controller.import(input.text, input.source));
      case 'config.enabled':
        if (typeof input.enabled !== 'boolean') throw new Error('Invalid protection setting.');
        return configView(await controller.setEnabled(input.enabled));
      case 'config.site':
        if (typeof input.host !== 'string' || typeof input.enabled !== 'boolean') throw new Error('Invalid site setting.');
        return configView(await controller.setSite(input.host, input.enabled));
      default: throw new Error('Unknown extension operation.');
    }
  };
}
