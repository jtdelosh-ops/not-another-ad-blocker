import { Controller, type Backend } from './controller';
import { createRouter } from './routes';
import { NativeClient } from '../shared/native-client';
import { RecoveryRequiredError, errorMessage } from '../shared/types';

const storageReady = chrome.storage.local.setAccessLevel({ accessLevel: 'TRUSTED_CONTEXTS' });
const backend: Backend = {
  async read() { await storageReady; return chrome.storage.local.get(['config', 'pending']); },
  async write(values) { await chrome.storage.local.set(values); },
  async remove(key) { await chrome.storage.local.remove(key); },
  rules: () => chrome.declarativeNetRequest.getDynamicRules(),
  replace: (removeRuleIds, addRules) => chrome.declarativeNetRequest.updateDynamicRules({ removeRuleIds, addRules }),
  maxDynamicRules: (chrome.declarativeNetRequest as typeof chrome.declarativeNetRequest & { MAX_NUMBER_OF_DYNAMIC_RULES?: number }).MAX_NUMBER_OF_DYNAMIC_RULES ?? 5_000,
  async notify() {
    const tabs = await chrome.tabs.query({ url: ['http://*/*', 'https://*/*'] });
    await Promise.allSettled(tabs.filter(tab => tab.id !== undefined).map(tab => chrome.tabs.sendMessage(tab.id!, { type: 'cosmetics.refresh' }, { frameId: 0 })));
  }
};
const native = new NativeClient((host, message) => chrome.runtime.sendNativeMessage(host, message as object));
const controller = new Controller(backend, native);
const route = createRouter(controller, chrome.runtime.id, chrome.runtime.getURL('/'));
chrome.runtime.onMessage.addListener((message: unknown, sender, respond) => {
  void route(message, sender).then(payload => respond({ ok: true, payload }), error => respond({ ok: false, error: errorMessage(error), code: error instanceof RecoveryRequiredError ? error.code : undefined }));
  return true;
});
