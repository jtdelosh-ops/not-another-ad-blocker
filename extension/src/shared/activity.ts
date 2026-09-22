export const ACTIVITY_LIMIT = 300;
export type ActivityAction = 'block' | 'allow' | 'allowAllRequests' | 'unknown';
export interface ActivityRuleInfo {
  action: ActivityAction;
  source: string;
  condition: string;
}
export interface ActivityEntry extends ActivityRuleInfo {
  id: number;
  timestamp: number;
  tabId: number;
  site: string | null;
  request: string;
  resourceType: string;
  ruleId: number;
}
export interface ActivityView {
  available: boolean;
  limit: number;
  entries: ActivityEntry[];
  tabId: number | null;
  site: string | null;
  blockedCount: string | null;
  error?: string;
}
export interface ActivityAccess {
  get(tabId: number | null): Promise<ActivityView>;
  clear(): Promise<void>;
}
