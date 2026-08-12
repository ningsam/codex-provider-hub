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

export interface QuotaWindow {
  remainingPercent: number;
  resetAfterSeconds: number;
}

export interface OfficialQuotaWindow {
  usedPercent: number;
  limitReached: boolean;
  resetAfterSeconds: number;
  resetAt: string | null;
}

export interface OfficialQuotaProbe {
  accountId: number;
  planType: string;
  allowed: boolean;
  limitReached: boolean;
  fiveHour: OfficialQuotaWindow | null;
  sevenDay: OfficialQuotaWindow | null;
  fetchedAt: string;
}

/** One real OpenAI/Codex OAuth account (not apikey 中转站). */
export interface Sub2ApiAccountQuota {
  id: number;
  name: string;
  email: string;
  /** oauth | relay | apikey | … */
  accountType: string;
  /** ready | error | inactive | … */
  status: string;
  errorMessage: string;
  fiveHour: QuotaWindow | null;
  sevenDay: QuotaWindow | null;
  schedulable: boolean;
  available: boolean;
  availability: string;
  availabilityReason: string;
  recoverable: boolean;
  unavailableUntil: string | null;
  preferred: boolean;
}

export interface Sub2ApiRoutingStatus {
  preferredAccountId: number | null;
  state:
    | "automatic"
    | "preferred"
    | "failover"
    | "unavailable"
    | "fallback_missing"
    | "unconfigured"
    | "stale"
    | "error";
  message: string;
  autoPauseThresholdPercent: number;
  policy: Sub2ApiRoutingPolicy;
  policyConfigured: boolean;
  recentWindowMinutes: number;
  recentRequestLimit: number;
  recentRequestCount: number;
  lastSuccessfulAccountId: number | null;
  lastSuccessfulAccountName: string | null;
  lastSuccessfulAccountType: string | null;
  lastSuccessfulAt: string | null;
  distribution: Sub2ApiRoutingDistribution[];
  oauthAvailableCount: number;
  relayAvailableCount: number;
  policyDeviation: boolean;
  policyDeviationMessage: string | null;
  activeRelayName: string | null;
}

export type Sub2ApiRoutingPolicy = "oauthFirst" | "relayFirst" | "balanced";

export interface Sub2ApiRoutingDistribution {
  accountId: number;
  name: string;
  accountType: string;
  requestCount: number;
  percent: number;
}

export interface Sub2ApiUsage {
  /** null when no OAuth account reports an active window */
  fiveHour: QuotaWindow | null;
  sevenDay: QuotaWindow | null;
  /** OAuth only — excludes AIHub/AnyRouter apikey relays */
  poolTotal: number;
  poolAvailable: number;
  accounts: Sub2ApiAccountQuota[];
  routing: Sub2ApiRoutingStatus;
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
