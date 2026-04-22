import { invoke } from '@tauri-apps/api/core';

// ── Types ────────────────────────────────────────────────────

export interface GatewayConnectionInfo {
  running: boolean;
  port: number;
  bindAddress: string;
  apiKey: string;
  requestTimeout: number;
  autoStart: boolean;
  adminPassword: string;
  enableLogging: boolean;
  lanIp: string | null;
  activeAccounts: number;
}

export interface SecurityConfig {
  mode: 'off' | 'whitelist' | 'blacklist';
  auto_ban_enabled: boolean;
  auto_ban_threshold: number;
  auto_ban_duration_secs: number;
  log_retention_days: number;
}

export interface WhitelistEntry {
  id: number;
  ip: string;
  description: string | null;
  created_at: number;
}

export interface BlacklistEntry {
  id: number;
  ip: string;
  reason: string | null;
  expires_at: number | null;
  created_at: number;
}

export interface AccessLogEntry {
  id: string;
  timestamp: number;
  client_ip: string | null;
  method: string;
  url: string;
  status: number;
  duration_ms: number;
  user_agent: string | null;
  api_key_prefix: string | null;
  model: string | null;
  account_id: string | null;
}

export interface AccessLogResponse {
  logs: AccessLogEntry[];
  total: number;
}

export interface IpRanking {
  client_ip: string;
  request_count: number;
  total_tokens: number;
  input_tokens: number;
  output_tokens: number;
  last_seen: number;
}

export interface IpStatsResponse {
  total_requests: number;
  unique_ips: number;
  blocked_requests: number;
  top_ips: IpRanking[];
}

// ── Gateway Connection ──────────────────────────────────────

export async function getGatewayConnectionInfo(): Promise<GatewayConnectionInfo> {
  return invoke<GatewayConnectionInfo>('get_gateway_connection_info');
}

export async function updateGatewayConfig(config: {
  bindAddress?: string;
  port?: number;
  apiKey?: string;
}): Promise<string> {
  return invoke<string>('update_gateway_config', { request: config });
}

export async function regenerateApiKey(): Promise<string> {
  return invoke<string>('regenerate_api_key');
}

export async function enableLanSharing(): Promise<string> {
  return invoke<string>('enable_lan_sharing');
}

export async function disableLanSharing(): Promise<string> {
  return invoke<string>('disable_lan_sharing');
}

// ── Security Config ─────────────────────────────────────────

export async function getSecurityConfig(): Promise<SecurityConfig> {
  return invoke<SecurityConfig>('get_security_config');
}

export async function updateSecurityConfig(config: SecurityConfig): Promise<void> {
  return invoke<void>('update_security_config', { config });
}

// ── Whitelist ───────────────────────────────────────────────

export async function listWhitelist(): Promise<WhitelistEntry[]> {
  return invoke<WhitelistEntry[]>('list_whitelist');
}

export async function addWhitelist(ip: string, description?: string): Promise<number> {
  return invoke<number>('add_whitelist', { ip, description: description ?? null });
}

export async function removeWhitelist(id: number, ip: string): Promise<boolean> {
  return invoke<boolean>('remove_whitelist', { id, ip });
}

// ── Blacklist ───────────────────────────────────────────────

export async function listBlacklist(): Promise<BlacklistEntry[]> {
  return invoke<BlacklistEntry[]>('list_blacklist');
}

export async function addBlacklist(
  ip: string,
  reason?: string,
  expiresAt?: number,
): Promise<number> {
  return invoke<number>('add_blacklist', {
    ip,
    reason: reason ?? null,
    expiresAt: expiresAt ?? null,
  });
}

export async function removeBlacklist(id: number, ip: string): Promise<boolean> {
  return invoke<boolean>('remove_blacklist', { id, ip });
}

// ── Access Logs ─────────────────────────────────────────────

export async function getAccessLogs(
  limit: number,
  offset: number,
  clientIp?: string,
  search?: string,
): Promise<AccessLogResponse> {
  return invoke<AccessLogResponse>('get_access_logs', {
    limit,
    offset,
    clientIp: clientIp ?? null,
    search: search ?? null,
  });
}

// ── IP Statistics ───────────────────────────────────────────

export async function getIpStatistics(hours?: number): Promise<IpStatsResponse> {
  return invoke<IpStatsResponse>('get_ip_statistics', { hours: hours ?? null });
}
