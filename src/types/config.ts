export interface AppConfig {
  ui: UiConfig;
  server_url: string;
  gateway: GatewayConfig;
  proxy: ProxyConfig;
  email: EmailConfig;
  sms: SmsConfig;
  registration: RegistrationConfig;
}

export interface UiConfig {
  language: string;
  theme: string;
  auto_launch: boolean;
  auto_check_update: boolean;
  update_check_interval: number;
  hidden_menu_items: string[];
  default_export_path?: string;
}

export interface GatewayConfig {
  enabled: boolean;
  port: number;
  bind_address: string;
  api_key: string;
  max_retries: number;
  request_timeout: number;
  safety_watermark: number;
  alert_watermark: number;
  auto_start: boolean;
  admin_password: string;
  enable_logging: boolean;
  vercel_api_key: string;
  // internal-only (present only on internal builds)
  transparent_enabled?: boolean;
  transparent_port?: number;
}

export interface ProxyConfig {
  default_type: string;
  residential: ResidentialProxyConfig;
  mobile: MobileProxyConfig;
}

export interface ResidentialProxyConfig {
  host: string;
  port: number;
  username: string;
  password: string;
  default_country: string;
}

export interface MobileProxyConfig {
  username: string;
  password: string;
  host: string;
  http_port: number;
  socks5_port: number;
  rotate_url: string;
}

export interface EmailConfig {
  domains: string[];
  mailslurp_api_key: string;
  mailslurp_inbox_id: string;
}

export interface SmsConfig {
  api_key: string;
}

export interface CountryConfig {
  sms_max_price: number;
}

export interface RegistrationConfig {
  supported_countries: Record<string, CountryConfig>;
}
