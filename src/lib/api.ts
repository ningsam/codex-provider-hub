import { invoke } from "@tauri-apps/api/core";
import type {
  AihubBalance,
  CursorAccount,
  CursorUsage,
  GatewayStatus,
  PickerGuardStatus,
  OfficialQuotaProbe,
  ProviderConfig,
  ProviderInfo,
  ProviderMutationResult,
  Sub2ApiBrowserLoginStatus,
  Sub2ApiImportResult,
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
  probeSub2apiOfficialQuota: (accountId: number) =>
    invoke<OfficialQuotaProbe>("probe_sub2api_official_quota", { accountId }),
  setSub2apiCurrentAccount: (accountId: number) =>
    invoke<Sub2ApiUsage>("set_sub2api_current_account", { accountId }),
  recoverSub2apiAccount: (accountId: number) =>
    invoke<Sub2ApiUsage>("recover_sub2api_account", { accountId }),
  setSub2apiAutoPauseThreshold: (threshold: number) =>
    invoke<Sub2ApiUsage>("set_sub2api_auto_pause_threshold", { percent: threshold }),
  setSub2apiRoutingPolicy: (policy: Sub2ApiUsage["routing"]["policy"]) =>
    invoke<Sub2ApiUsage>("set_sub2api_routing_policy", { policy }),
  deleteSub2apiAccount: (accountId: number) =>
    invoke<void>("delete_sub2api_account", { accountId }),
  importSub2apiFile: (filePath: string, name?: string) =>
    invoke<Sub2ApiImportResult>("import_sub2api_file", {
      filePath,
      name: name ?? null,
    }),
  beginSub2apiBrowserLogin: () =>
    invoke<Sub2ApiBrowserLoginStatus>("begin_sub2api_browser_login"),
  getSub2apiBrowserLoginStatus: (sessionId: string) =>
    invoke<Sub2ApiBrowserLoginStatus>("get_sub2api_browser_login_status", { sessionId }),
  completeSub2apiBrowserLogin: (sessionId: string, name?: string) =>
    invoke<Sub2ApiBrowserLoginStatus>("complete_sub2api_browser_login", {
      sessionId,
      name: name ?? null,
    }),
  cancelSub2apiBrowserLogin: (sessionId: string) =>
    invoke<void>("cancel_sub2api_browser_login", { sessionId }),

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
