export function element<T extends HTMLElement = HTMLElement>(id: string): T {
  const value = document.getElementById(id);
  if (!value) throw new Error(`Missing interface element: ${id}`);
  return value as T;
}
export async function request<T>(message: unknown): Promise<T> {
  const response = await chrome.runtime.sendMessage(message);
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
