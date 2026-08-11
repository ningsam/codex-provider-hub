export type RouteDoctorSeverity = "info" | "warning" | "critical";

export type RouteDoctorRepairAction =
  | { action: "setAccountsSchedulable"; accountIds: number[] }
  | { action: "clearModelRateLimits"; accountIds: number[] }
  | { action: "resetTransientParking"; accountIds: number[] }
  | {
      action: "setGroupSupportedScopes";
      groupId: number;
      scopes: string[];
    }
  | {
      action: "setGroupRequireOauthOnly";
      groupId: number;
      enabled: boolean;
    }
  | {
      action: "addModelMapping";
      accountIds: number[];
      clientModel: string;
      upstreamModel: string;
    }
  | {
      action: "ensureRelayFallback";
      groupId: number;
      relayAccountIds: number[];
      priority: number;
    }
  | {
      action: "moveApiKeyToGroup";
      apiKeyId: number;
      targetGroupId: number;
    }
  | { action: "setApiKeyActive"; apiKeyId: number }
  | { action: "setGroupActive"; groupId: number };

export interface RouteDoctorIssue {
  code: string;
  severity: RouteDoctorSeverity;
  title: string;
  detail: string;
  accountIds: number[];
  groupId: number | null;
  repair: RouteDoctorRepairAction | null;
}

export interface RouteDoctorReport {
  healthy: boolean;
  currentApiKeyId: number | null;
  currentGroupId: number | null;
  currentGroupName: string | null;
  currentModel: string;
  usableMemberCount: number;
  issues: RouteDoctorIssue[];
  generatedAt: string;
}

export interface RouteDoctorProbeCheck {
  attempted: boolean;
  success: boolean;
  statusCode: number | null;
  detail: string;
}

export interface RouteDoctorRelayProbe {
  accountId: number;
  accountName: string;
  upstreamHost: string;
  models: RouteDoctorProbeCheck;
  responses: RouteDoctorProbeCheck;
}

export interface RouteDoctorResult {
  report: RouteDoctorReport;
  relayProbes: RouteDoctorRelayProbe[];
  capturedAt: string;
}

export interface RouteDoctorPlannedChange {
  entity: string;
  entityId: number;
  field: string;
  oldValue: unknown;
  newValue: unknown;
}

export interface RouteDoctorRepairPlan {
  action: RouteDoctorRepairAction;
  summary: string;
  changes: RouteDoctorPlannedChange[];
  backupRequired: boolean;
  appRestartRequired: boolean;
}

export interface RouteDoctorRepairResult {
  plan: RouteDoctorRepairPlan;
  applied: boolean;
  backupPath: string | null;
  requestId: string | null;
  message: string;
}
