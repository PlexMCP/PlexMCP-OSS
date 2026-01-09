---
sidebar_position: 1
---

# API Key Authentication

Learn how to authenticate with PlexMCP using API keys.

## Overview

API keys are the primary way to authenticate with PlexMCP. Each key:
- Is scoped to your organization
- Can access specific MCPs
- Has optional expiration
- Tracks usage independently

## Getting Your API Key

### From the Dashboard

1. Log in to [dashboard.plexmcp.com](https://dashboard.plexmcp.com)
2. Navigate to **API Keys**
3. Click **Create API Key**
4. Configure your key:
   - **Name**: Descriptive identifier
   - **Description**: Usage notes
   - **Expiration**: When it expires
   - **Permissions**: Which MCPs to access
5. Click **Create**
6. Copy the key immediately

:::warning Important
Your API key is only shown once. Copy it before closing the dialog.
:::

## Using Your API Key

### HTTP Header

Include the key in the Authorization header:

```bash
curl -X POST https://api.plexmcp.com/mcp \
  -H "Authorization: ApiKey pmcp_xxxxxxxx" \
  -H "Content-Type: application/json" \
  -d '{...}'
```

### SDK Initialization

```typescript
import { PlexMCP } from '@plexmcp/sdk';

const client = new PlexMCP({
  apiKey: 'pmcp_xxxxxxxx'
});
```

### Environment Variables

Best practice is to use environment variables:

```bash
# .env file
PLEXMCP_API_KEY=pmcp_xxxxxxxx
```

```typescript
const client = new PlexMCP({
  apiKey: process.env.PLEXMCP_API_KEY
});
```

## Key Permissions

### All MCPs

Grant access to all current and future MCPs:

```
Permissions: All MCPs
```

### Specific MCPs

Limit access to selected MCPs only:

```
Permissions: Weather API, Calculator
```

## Key Expiration

### Never Expire

For long-term production keys. Use with rotation policy.

### Set Expiration

Common expiration periods:
- **30 days**: Contractor access
- **90 days**: Regular rotation
- **1 year**: Long-term with review

## Security Best Practices

### 1. Use Environment Variables

Never hardcode keys:

```typescript
// Bad
const apiKey = "pmcp_xxxxx";

// Good
const apiKey = process.env.PLEXMCP_API_KEY;
```

### 2. Don't Commit Keys

Add to `.gitignore`:

```
.env
.env.local
*.env
```

### 3. Use Separate Keys

Create different keys for:
- Production
- Staging
- Development
- CI/CD

### 4. Minimal Permissions

Only grant access to needed MCPs.

### 5. Rotate Regularly

Set a rotation schedule:
1. Create new key
2. Update applications
3. Verify new key works
4. Revoke old key

### 6. Monitor Usage

Check dashboard for:
- Unusual request volume
- Unexpected error rates
- Unknown client IPs

## Revoking Keys

If a key is compromised:

1. Go to **API Keys**
2. Find the key
3. Click **Revoke**
4. Confirm revocation

The key stops working immediately.

## Troubleshooting

### Invalid API Key

**Error**: `401 Unauthorized - Invalid or missing API key`

**Causes**:
- Missing Authorization header
- Wrong key format
- Key revoked or expired

**Fix**: Verify the complete key is included with `ApiKey` prefix.

### Key Not Working

1. Check key was copied completely
2. Verify no extra whitespace
3. Confirm key isn't revoked
4. Check expiration date

### MCP Access Denied

**Error**: `403 Forbidden - API key does not have access to this MCP`

**Fix**: Update key permissions to include the MCP, or create a new key with access.
