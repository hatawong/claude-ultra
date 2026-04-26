#!/usr/bin/env bun
/**
 * Migrate accounts/*.json from V1 flat format to V3 nested format.
 *
 * - Moves sessionKey / routingHint / device into android sub-object
 * - Removes registrationId
 * - Adds V3 default fields (subscriptionType, disabled, etc.)
 * - Backs up originals to accounts.v1-backup/
 * - Idempotent: skips files that already have android sub-object
 *
 * Usage: bun run scripts/migrate-v1-to-v3.ts [--dry-run]
 */

import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync, copyFileSync } from 'fs';
import { join, basename } from 'path';

const ACCOUNTS_DIR = join(process.env.HOME!, '.claude-ultra', 'accounts');
const BACKUP_DIR = join(process.env.HOME!, '.claude-ultra', 'accounts.v1-backup');
const DRY_RUN = process.argv.includes('--dry-run');

function main() {
  if (!existsSync(ACCOUNTS_DIR)) {
    console.log(`No accounts directory: ${ACCOUNTS_DIR}`);
    process.exit(0);
  }

  const files = readdirSync(ACCOUNTS_DIR).filter(f => f.endsWith('.json'));
  if (files.length === 0) {
    console.log('No .json files found in accounts/');
    process.exit(0);
  }

  // Ensure backup directory exists
  if (!DRY_RUN) {
    mkdirSync(BACKUP_DIR, { recursive: true });
  }

  let migrated = 0;
  let skipped = 0;

  for (const file of files) {
    const filePath = join(ACCOUNTS_DIR, file);
    const raw = readFileSync(filePath, 'utf-8');
    let account: any;
    try {
      account = JSON.parse(raw);
    } catch {
      console.log(`  SKIP ${file} (invalid JSON)`);
      skipped++;
      continue;
    }

    // Idempotent: skip if already V3 (has android sub-object)
    if (account.android !== undefined) {
      console.log(`  SKIP ${file} (already V3)`);
      skipped++;
      continue;
    }

    // Must have sessionKey + device to migrate
    if (!account.sessionKey || !account.device) {
      console.log(`  SKIP ${file} (missing sessionKey or device)`);
      skipped++;
      continue;
    }

    console.log(`  MIGRATE ${file}: ${account.accountId}`);

    // Backup original
    if (!DRY_RUN) {
      copyFileSync(filePath, join(BACKUP_DIR, file));
    }

    // Build V3 structure
    const v3: any = {
      accountId: account.accountId,
      email: account.email,
      fullName: account.fullName || '',
      phoneNumber: account.phoneNumber || '',
      customLabel: null,
      accountUuid: account.accountUuid || '',
      orgId: account.orgId || '',
      region: account.region || '',
      createdAt: account.createdAt || 0,

      // Plan defaults
      subscriptionType: account.plan || 'free',
      rateLimitTier: null,
      subscriptionRenewAt: null,
      subscriptionCreatedAt: null,
      billingType: null,
      hasExtraUsageEnabled: false,

      // Status defaults
      disabled: false,
      disabledReason: null,
      disabledAt: null,
      userDisabled: false,
      userDisabledReason: null,
      userDisabledAt: null,

      // Proxy
      proxy: null,

      // Quota
      quota: null,

      // Android — moved from top-level
      android: {
        device: account.device,
        sessionKey: account.sessionKey,
        routingHint: account.routingHint || '',
        lastActivity: null,
      },

      // Web and CLI — null (not yet authorized)
      web: account.web || null,
      cli: null,
    };

    if (!DRY_RUN) {
      writeFileSync(filePath, JSON.stringify(v3, null, 2) + '\n');
    }

    migrated++;
  }

  console.log(`\nDone: ${migrated} migrated, ${skipped} skipped${DRY_RUN ? ' (dry run)' : ''}`);
  if (migrated > 0 && !DRY_RUN) {
    console.log(`Backups saved to: ${BACKUP_DIR}`);
  }
}

main();
