// Static proxy paste parser. Mirrors shared/src/proxy.ts::parseStaticProxyPasted.
// Cannot import shared module directly: shared/src/account.ts pulls in fs/path
// which breaks the manager renderer build (no nodejs types).
//
// If shared is later refactored to split types from server-side helpers, this
// file should re-export shared's parser instead.

import type { StaticProxy, StaticProtocol } from '../types/account';

const MAX_PASTED_LEN = 1024;

/**
 * Parse pasted static proxy credentials. Accepts both formats:
 *   - Non-standard IPFoxy backend: socks5://host:port:user:pass
 *   - Standard URL:                socks5://user:pass@host:port
 * Returns null on invalid input.
 *
 * Safety: rejects inputs >1024 chars or containing control chars to prevent
 * regex DoS / injection.
 */
export function parseStaticProxyPasted(s: string): StaticProxy | null {
  const t = s.trim();
  if (t.length === 0 || t.length > MAX_PASTED_LEN) return null;
  if (/[\x00-\x1f\x7f]/.test(t)) return null;
  // Non-standard IPFoxy backend format: {proto}://host:port:user:pass
  const nonStd = t.match(/^(socks5|http):\/\/([^:\/]+):(\d+):([^:]+):(.+)$/);
  if (nonStd) {
    return {
      protocol: nonStd[1] as StaticProtocol,
      host: nonStd[2]!,
      port: parseInt(nonStd[3]!, 10),
      username: nonStd[4]!,
      password: nonStd[5]!,
    };
  }
  // Standard URL: {proto}://user:pass@host:port
  const std = t.match(/^(socks5|http):\/\/([^:@]+):([^@]+)@([^:\/]+):(\d+)$/);
  if (std) {
    let username: string;
    let password: string;
    try {
      username = decodeURIComponent(std[2]!);
      password = decodeURIComponent(std[3]!);
    } catch {
      return null;
    }
    return {
      protocol: std[1] as StaticProtocol,
      host: std[4]!,
      port: parseInt(std[5]!, 10),
      username,
      password,
    };
  }
  return null;
}
