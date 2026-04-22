export interface AuthStatus {
  mode: 'internal' | 'distribution';
  status: 'logged_in' | 'not_logged_in';
  user?: AuthUser;
}

export interface AuthUser {
  id: string;
  displayName: string;
  plan: string;
  maxAccounts: number;
  planExpiresAt?: number;
}

export interface DeviceFlowStart {
  userCode: string;
  verificationUri: string;
  deviceCode: string;
  interval: number;
  expiresIn: number;
}

export interface DeviceFlowResult {
  user: AuthUser;
}
