export function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const value = document.getElementById(id);
  if (!value) throw new Error(`Missing interface element: ${id}`);
  return value as T;
}
export async function request<T>(message: unknown): Promise<T> {
  const type = (message as { type?: string } | null)?.type;
  const mutation = ['config.import', 'config.enabled', 'config.site', 'subscriptions.refresh', 'subscriptions.remove', 'picker.save', 'picker.undo'].includes(type ?? '');
  // List downloads have a longer native deadline than local compilation.
  const timeoutMs = type === 'subscriptions.refresh' ? 180_000 : 30_000;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let response;
  try {
    response = await Promise.race([
      chrome.runtime.sendMessage(message),
      new Promise<never>((_, reject) => {
        timer = setTimeout(() => reject(new Error('The extension did not respond in time.')), timeoutMs);
      })
    ]);
  } catch (error) {
    // A closed channel/timeout does not tell us whether a mutation committed.
    // Never retry it automatically or render the old config as confirmed.
    const detail = error instanceof Error ? error.message : String(error);
    if (mutation) throw new RecoveryRequiredError(`The extension did not confirm the change. It may already be saved. Copy any unsaved filter text, then reload NAAB from the browser extensions page and reopen this page to verify the saved rules. ${detail}`);
    throw new Error(`Could not get a reply from NAAB. Reload the extension and try again. ${detail}`);
  } finally {
    if (timer !== undefined) clearTimeout(timer);
  }
  if (!response || typeof response.ok !== 'boolean') {
    if (mutation) throw new RecoveryRequiredError('The extension did not confirm the change. It may already be saved. Copy any unsaved filter text, then reload NAAB from the browser extensions page and reopen this page to verify the saved rules.');
    throw new Error('The extension did not respond.');
  }
  if (!response || response.ok !== true) {
    const message = response?.error ?? 'The extension did not respond.';
    throw response?.code === 'RECOVERY_REQUIRED' ? new RecoveryRequiredError(message) : new Error(message);
  }
  return response.payload as T;
}
export function renderRecoveryRequired(summary: Pick<HTMLElement, 'textContent'>, mutationControls: Iterable<{ disabled: boolean }>): void {
  summary.textContent = 'Protection state is unknown — reload the extension to recover';
  for (const control of mutationControls) control.disabled = true;
}
export function pollSubscriptionProgress(read: () => Promise<SubscriptionProgress>, render: (progress: SubscriptionProgress) => void, intervalMs = 500): () => void {
  let active = true;
  const timer = setInterval(() => {
    void read().then(progress => { if (active) render(progress); }, () => {});
  }, intervalMs);
  return () => { active = false; clearInterval(timer); };
}
import { RecoveryRequiredError, type SubscriptionProgress } from './types';
