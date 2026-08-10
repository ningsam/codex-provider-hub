import { invoke } from "@tauri-apps/api/core";
import type {
  AihubBalance,
  CursorAccount,
  CursorUsage,
  GatewayStatus,
  PickerGuardStatus,
  ProviderConfig,
  ProviderInfo,
  ProviderMutationResult,
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
  deleteSub2apiAccount: (accountId: number) =>
    invoke<void>("delete_sub2api_account", { accountId }),

  getAihubBalance: () => invoke<AihubBalance>("get_aihub_balance"),
  setAihubApiKey: (apiKey: string) =>
    invoke<AihubBalance>("set_aihub_api_key", { apiKey }),
  clearAihubApiKey: () => invoke<void>("clear_aihub_api_key"),

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

  listProviders: () => invoke<ProviderInfo[]>("list_providers"),
  addProvider: (args: {
    name: string;
    baseUrl: string;
    apiKey: string;
    prefix?: string;
    probeModels?: boolean;
  }) =>
    invoke<ProviderMutationResult>("add_provider", {
      name: args.name,
      baseUrl: args.baseUrl,
      apiKey: args.apiKey,
      prefix: args.prefix ?? null,
      probeModels: args.probeModels ?? true,
    }),
  removeProvider: (accountId: number) =>
    invoke<ProviderInfo>("remove_provider", { accountId }),
  syncProviderModels: (accountId: number, apiKey?: string) =>
    invoke<ProviderMutationResult>("sync_provider_models", {
      accountId,
      apiKey: apiKey ?? null,
    }),
  probeProviderModels: (baseUrl: string, apiKey: string) =>
    invoke<string[]>("probe_provider_models", { baseUrl, apiKey }),

  getPickerGuardStatus: () => invoke<PickerGuardStatus>("get_picker_guard_status"),
  applyPickerGuard: () => invoke<PickerGuardStatus>("apply_picker_guard"),
  relaunchChatgptGuarded: () =>
    invoke<PickerGuardStatus>("relaunch_chatgpt_guarded"),
  openChatgptGuarded: () => invoke<PickerGuardStatus>("open_chatgpt_guarded"),
  setPickerGuardEnabled: (enabled: boolean) =>
    invoke<PickerGuardStatus>("set_picker_guard_enabled", { enabled }),
};

/** Staggered card refresh intervals (ms). */
export const REFRESH_MS = {
  gateway: 15_000,
  sub2api: 60_000,
  aihub: 120_000,
  cursor: 120_000,
  providers: 60_000,
  pickerGuard: 30_000,
} as const;
