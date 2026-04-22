import { invoke } from '@tauri-apps/api/core';

export interface RequestLog {
  id: string;
  timestamp: number;
  method: string;
  url: string;
  status: number;
  duration_ms: number;
  model: string | null;
  account_id: string | null;
  account_email: string | null;
  input_tokens: number | null;
  output_tokens: number | null;
  cache_creation_tokens: number | null;
  cache_read_tokens: number | null;
  total_tokens: number | null;
  input_cost: number | null;
  output_cost: number | null;
  cache_creation_cost: number | null;
  cache_read_cost: number | null;
  total_cost: number | null;
  error: string | null;
  request_size: number | null;
  response_size: number | null;
  client_ip: string | null;
  user_agent: string | null;
  api_key_prefix: string | null;
  request_body: string | null;
  response_body: string | null;
}

export interface TokenStatsRow {
  period: string;
  total_input_tokens: number;
  total_output_tokens: number;
  total_tokens: number;
  total_cost: number;
  request_count: number;
}

export interface CostSummary {
  total_requests: number;
  total_input_tokens: number;
  total_output_tokens: number;
  total_tokens: number;
  total_cost: number;
  by_model: ModelCostRow[];
}

export interface ModelCostRow {
  model: string;
  request_count: number;
  total_tokens: number;
  total_cost: number;
}

export async function getRequestLogs(
  limit: number,
  offset: number,
  accountId?: string,
  model?: string,
  dateFrom?: number,
  dateTo?: number,
  search?: string,
): Promise<RequestLog[]> {
  return invoke<RequestLog[]>('get_request_logs', {
    limit, offset,
    accountId: accountId ?? null,
    model: model ?? null,
    dateFrom: dateFrom ?? null,
    dateTo: dateTo ?? null,
    search: search ?? null,
  });
}

export async function getTokenStats(
  period: 'hour' | 'day' | 'week',
  accountId?: string,
): Promise<TokenStatsRow[]> {
  return invoke<TokenStatsRow[]>('get_token_stats', {
    period,
    accountId: accountId ?? null,
  });
}

export async function getCostSummary(
  dateFrom: number,
  dateTo: number,
): Promise<CostSummary> {
  return invoke<CostSummary>('get_cost_summary', { dateFrom, dateTo });
}

export async function getLogsCount(search?: string): Promise<number> {
  return invoke<number>('get_logs_count', { search: search ?? null });
}

export async function clearGatewayLogs(beforeTimestamp?: number): Promise<number> {
  return invoke<number>('clear_gateway_logs', {
    beforeTimestamp: beforeTimestamp ?? null,
  });
}

export async function getLogDetail(id: string): Promise<RequestLog> {
  return invoke<RequestLog>('get_log_detail', { id });
}

export async function setLoggingEnabled(enabled: boolean): Promise<void> {
  return invoke<void>('set_logging_enabled', { enabled });
}
