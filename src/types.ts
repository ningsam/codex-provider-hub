/** Mirrors Rust serde types (camelCase) in src-tauri/src/*.rs */

export interface GatewayStatus {
  running: boolean;
  healthy: boolean;
  port: number;
  providerCount: number;
  modelCount: number;
  lastCheckedAt: string;
}

export interface ModelRoute {
  id: string;
  prefix: string;
  upstream: string;
  enabled: boolean;
}

export interface ProviderConfig {
  listenAddr: string;
  providers: string[];
  models: ModelRoute[];
  updatedAt: string;
}

export type RoutingTargetKind = "official" | "pool" | "oauth" | "provider";

export interface RoutingTarget {
  id: string;
  kind: RoutingTargetKind;
  accountId: number | null;
  name: string;
  detail: string;
  available: boolean;
  selected: boolean;
}

export interface RoutingState {
  activeTarget: string;
  modelProvider: string;
  targets: RoutingTarget[];
  gatewayError: string | null;
  updatedAt: string;
}

export interface QuotaWindow {
  remainingPercent: number;
  resetAfterSeconds: number;
}

/** One real OpenAI/Codex OAuth account (not apikey 中转站). */
export interface Sub2ApiAccountQuota {
  id: number;
  name: string;
  email: string;
  /** ready | error | inactive | … */
  status: string;
  errorMessage: string;
  fiveHour: QuotaWindow | null;
  sevenDay: QuotaWindow | null;
  schedulable: boolean;
}

export interface Sub2ApiUsage {
  /** null when no OAuth account reports an active window */
  fiveHour: QuotaWindow | null;
  sevenDay: QuotaWindow | null;
  /** OAuth only — excludes AIHub/AnyRouter apikey relays */
  poolTotal: number;
  poolAvailable: number;
  accounts: Sub2ApiAccountQuota[];
  fetchedAt: string;
}

export interface Sub2ApiImportResult {
  created: number;
  updated: number;
  skipped: number;
  failed: number;
  summary: string;
}

export interface Sub2ApiBrowserLoginStatus {
  sessionId: string | null;
  loginUrl: string;
  state: "waiting" | "ready" | "complete" | "expired" | "cancelled";
  message: string;
  importedAccounts: string[];
}

export interface AihubBalance {
  balance: number;
  used: number;
  currency: string;
  fetchedAt: string;
  /** Which credential source produced this snapshot */
  keySource: string;
  hasStoredKey: boolean;
}

export interface CursorAccount {
  id: string;
  email: string;
  accessToken: string;
  createdAt: string;
}

export interface CursorUsage {
  accountId: string;
  email: string;
  planName: string;
  planLimit: number;
  used: number;
  remaining: number;
  autoPercent: number;
  apiPercent: number;
  totalPercent: number;
  fetchedAt: string;
}

export interface ProviderInfo {
  id: number;
  name: string;
  baseUrl: string;
  baseUrlMasked: string;
  prefix: string;
  status: string;
  modelCount: number;
  hasApiKey: boolean;
  schedulable: boolean;
  errorMessage: string;
}

export interface ProviderMutationResult {
  provider: ProviderInfo;
  modelsSynced: number;
  modelIds: string[];
  allowlistUpdated: boolean;
  restartRequired: boolean;
  hint?: string | null;
}

export interface PickerGuardStatus {
  enabled: boolean;
  useHiddenModels: boolean | null;
  patchedAt: string | null;
  chatgptRunning: boolean;
  /** ChatGPT main process cmdline includes Statsig --host-rules */
  hostRulesActive: boolean;
  leveldbPath: string;
  lastError: string | null;
  pendingFix: boolean;
}
