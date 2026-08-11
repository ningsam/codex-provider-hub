import { invoke } from "@tauri-apps/api/core";
import type {
  RouteDoctorRelayProbe,
  RouteDoctorRepairAction,
  RouteDoctorRepairResult,
  RouteDoctorResult,
} from "../routeDoctorTypes";

export const routeDoctorApi = {
  diagnose: () =>
    invoke<RouteDoctorResult>("diagnose_sub2api_route"),

  probeRelays: (probeResponses = false) =>
    invoke<RouteDoctorRelayProbe[]>("probe_sub2api_route_relays", {
      probeResponses,
    }),

  repair: (
    action: RouteDoctorRepairAction,
    apply = false,
    confirmation?: string,
  ) =>
    invoke<RouteDoctorRepairResult>("repair_sub2api_route", {
      action,
      apply,
      confirmation: confirmation ?? null,
    }),
};
