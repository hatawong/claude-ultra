import { invoke } from '@tauri-apps/api/core';
import type { AppConfig } from '../types/config';

export async function loadConfig(): Promise<AppConfig> {
  return await invoke<AppConfig>('load_config');
}

export async function saveConfig(config: AppConfig): Promise<void> {
  return await invoke('save_config', { config });
}
