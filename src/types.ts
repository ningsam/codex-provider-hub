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

export interface Sub2ApiUsage {
  fiveHour: QuotaWindow;
  sevenDay: QuotaWindow;
  poolTotal: number;
  poolAvailable: number;
  fetchedAt: string;
}

export interface AihubBalance {
  balance: number;
  used: number;
  currency: string;
  fetchedAt: string;
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
