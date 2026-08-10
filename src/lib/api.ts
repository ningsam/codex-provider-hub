import { invoke } from "@tauri-apps/api/core";
import type {
  AihubBalance,
  CursorAccount,
  CursorUsage,
  GatewayStatus,
  ProviderConfig,
  Sub2ApiUsage,
} from "../types";

export const api = {
  getGatewayStatus: () => invoke<GatewayStatus>("get_gateway_status"),
  startGateway: () => invoke<GatewayStatus>("start_gateway"),
  stopGateway: () => invoke<GatewayStatus>("stop_gateway"),
  getProviderConfig: () => invoke<ProviderConfig>("get_provider_config"),
  saveProviderConfig: (cfg: ProviderConfig) =>
    invoke<ProviderConfig>("save_provider_config", { cfg }),

  getSub2apiUsage: () => invoke<Sub2ApiUsage>("get_sub2api_usage"),

  getAihubBalance: () => invoke<AihubBalance>("get_aihub_balance"),

  listCursorAccounts: () => invoke<CursorAccount[]>("list_cursor_accounts"),
  addCursorAccount: (email: string, accessToken: string) =>
    invoke<CursorAccount>("add_cursor_account", {
      email,
      accessToken,
    }),
  importLocalCursorAccount: () =>
    invoke<CursorAccount>("import_local_cursor_account"),
  removeCursorAccount: (id: string) =>
    invoke<void>("remove_cursor_account", { id }),
  getCursorUsage: (id: string) =>
    invoke<CursorUsage>("get_cursor_usage", { id }),
};

/** Staggered card refresh intervals (ms). */
export const REFRESH_MS = {
  gateway: 15_000,
  sub2api: 60_000,
  aihub: 120_000,
  cursor: 120_000,
} as const;
