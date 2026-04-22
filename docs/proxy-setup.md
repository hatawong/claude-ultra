# Residential Proxy Setup (IPRoyal)

Claude Ultra supports residential proxy rotation to avoid IP-based rate limiting. This guide walks you through setting up IPRoyal residential proxies.

## Step 1: Create an IPRoyal Account

1. Go to [https://iproyal.com](https://iproyal.com/?r=claude-ultra)
2. Sign up for an account
3. Navigate to **Residential Proxies** in the dashboard

## Step 2: Purchase Residential Proxy Traffic

1. In the IPRoyal dashboard, go to **Residential Proxies → Purchase**
2. Choose a traffic plan (pay-per-GB)
   - Recommended starting amount: **1 GB** (~$7) for testing
   - Traffic does not expire
3. Complete the payment

## Step 3: Get Your Credentials

1. In the IPRoyal dashboard, go to **Residential Proxies → Setup**
2. You will see:
   - **Username** (e.g. `customer-xxxx`)
   - **Password** (a long string)
3. Keep these credentials — you'll enter them in Claude Ultra

## Step 4: Configure in Claude Ultra

1. Open Claude Ultra → **Settings → Proxy**
2. Enable **Residential Proxy**
3. Fill in:

| Field | Value |
|-------|-------|
| Host | `geo.iproyal.com` (default, no need to change) |
| Port | `12321` (default, no need to change) |
| Username | Your IPRoyal username (e.g. `customer-xxxx`) |
| Password | Your IPRoyal password |
| Country | `us` (default) or any [supported country code](https://iproyal.com/locations/) |

4. Click **Save**

Or edit `~/.claude-ultra/config.json` directly:

```json
{
  "proxy": {
    "default_type": "residential",
    "residential": {
      "host": "geo.iproyal.com",
      "port": 12321,
      "username": "customer-xxxx",
      "password": "your_password_here",
      "default_country": "us"
    }
  }
}
```

## Step 5: Verify

After saving, Claude Ultra will route API requests through IPRoyal residential proxies. Each account session gets a sticky IP (fixed for 24 hours), and IPs rotate automatically between sessions.

You can verify proxy usage in **Traffic Logs** — the proxy column will show the assigned IP.

## How It Works

- Each Claude account is assigned a **sticky residential IP** for 24 hours
- When accounts rotate, a new session ID is generated → new IP
- Traffic is billed per GB by IPRoyal (not by Claude Ultra)
- If proxy credentials are empty, Claude Ultra connects directly (no proxy)

## Supported Countries

IPRoyal supports 195+ countries. Common choices:

| Country | Code |
|---------|------|
| United States | `us` |
| United Kingdom | `gb` |
| Germany | `de` |
| Japan | `jp` |
| Singapore | `sg` |

Full list: [https://iproyal.com/locations/](https://iproyal.com/locations/)

## Troubleshooting

**"Proxy authentication failed"**
- Double-check username and password in Settings → Proxy
- Make sure you purchased **Residential Proxies** (not Datacenter or ISP)

**"Connection timeout via proxy"**
- Check your IPRoyal balance (0 GB remaining = connection refused)
- Try a different country code

**Requests work without proxy but fail with proxy**
- Some regions have slower residential IPs — try `us` or `gb`
- Check IPRoyal dashboard for service status
