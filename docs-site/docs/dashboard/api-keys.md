---
sidebar_position: 3
---

# API Key Management

API keys are how you authenticate with PlexMCP. This guide covers creating, managing, and securing your keys.

## Understanding API Keys

Each API key:
- Identifies your organization
- Grants access to specific MCPs
- Can have an expiration date
- Tracks usage independently
- Can be revoked instantly

## Creating an API Key

1. Navigate to **API Keys** in the sidebar
2. Click **Create API Key**
3. Configure the key:

   | Field | Description |
   |-------|-------------|
   | **Name** | Descriptive name (e.g., "Production Server") |
   | **Description** | Optional notes about usage |
   | **Expiration** | When the key expires (or never) |
   | **MCP Access** | Which MCPs this key can use |

4. Click **Create**
5. **Copy the key immediately** - it won't be shown again

:::warning
Your API key is only shown once. Store it securely before closing the dialog.
:::

## Key Permissions

Each key can access specific MCPs:

### All MCPs
Grant access to every MCP in your organization. New MCPs added later will also be accessible.

### Specific MCPs
Select individual MCPs. The key can only access those you choose.

## Using API Keys

Include your API key in the `Authorization` header:

```bash
curl -X POST https://api.plexmcp.com/mcp \
  -H "Authorization: ApiKey YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"mcp_id": "...", "tool": "...", "arguments": {}}'
```

Or in code:

```typescript
const response = await fetch('https://api.plexmcp.com/mcp', {
  method: 'POST',
  headers: {
    'Authorization': `ApiKey ${apiKey}`,
    'Content-Type': 'application/json',
  },
  body: JSON.stringify({
    mcp_id: 'your-mcp-id',
    tool: 'tool-name',
    arguments: {},
  }),
});
```

## Viewing Your Keys

The API Keys page shows:

| Column | Description |
|--------|-------------|
| **Name** | Key identifier |
| **Created** | When the key was made |
| **Last Used** | Most recent activity |
| **Expires** | Expiration date (or Never) |
| **Status** | Active or Revoked |

Click a key to see:
- Full details
- Usage statistics
- Activity log
- MCP permissions

## Revoking a Key

If a key is compromised or no longer needed:

1. Find the key in the list
2. Click the **Revoke** button
3. Confirm the action

Revoked keys:
- Stop working immediately
- Cannot be reactivated
- Remain in the list for audit purposes

## Key Best Practices

### Use Separate Keys for Each Environment

```
Production API Key    → prod-server
Staging API Key       → staging-server
Development API Key   → local-dev
```

### Set Expiration Dates

For temporary access or contractors, set expiration:
- 30 days for short-term access
- 90 days for regular rotation
- 1 year for long-term keys

### Limit MCP Access

Only grant access to the MCPs each key needs:
- Production keys: Only production MCPs
- Testing keys: Only test MCPs

### Rotate Keys Regularly

Create new keys and phase out old ones:
1. Create a new key
2. Update your applications
3. Verify the new key works
4. Revoke the old key

### Never Commit Keys to Git

Use environment variables instead:

```bash
# .env (not committed)
PLEXMCP_API_KEY=pmcp_...

# In your code
const apiKey = process.env.PLEXMCP_API_KEY;
```

### Monitor Key Usage

Check the dashboard for:
- Unusual request volumes
- Requests from unexpected locations
- Error rate spikes

## Key Limits

Based on your plan:

| Plan | API Keys |
|------|----------|
| Free | 5 |
| Pro | 20 |
| Team | 50 |
| Enterprise | Unlimited |

## Troubleshooting

### "Invalid API Key" Error

- Verify you copied the complete key
- Check the key hasn't been revoked
- Ensure the key hasn't expired
- Confirm you're using the right organization's key

### "Unauthorized MCP" Error

- Verify the key has permission for that MCP
- Check the MCP hasn't been deleted
- Update key permissions if needed

### Key Not Working After Creation

- Keys are active immediately
- Check for typos when copying
- Verify you're using the correct API endpoint
