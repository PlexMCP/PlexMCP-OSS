---
sidebar_position: 6
---

# Organization Settings

Configure your organization's preferences and security settings.

## Accessing Settings

1. Navigate to **Settings** in the sidebar
2. Only Owners and Admins can access most settings

## General Settings

### Organization Name
The display name for your organization:
1. Click **Edit** next to the name
2. Enter the new name
3. Click **Save**

### Organization Slug
The unique identifier used in URLs and API calls:
- Format: lowercase letters, numbers, and hyphens
- Example: `my-company`
- **Cannot be changed** after creation

### Organization Description
Optional description for your team:
1. Click **Edit Description**
2. Enter your description
3. Click **Save**

## Security Settings

### PIN Protection

Add an extra layer of security for sensitive actions:

1. Go to **Settings** → **Security**
2. Toggle **PIN Protection** on
3. Set a 6-digit PIN
4. Confirm your PIN

With PIN protection enabled, you'll need to enter your PIN for:
- Deleting MCPs
- Revoking API keys
- Removing team members
- Changing billing

### Two-Factor Authentication

Enable 2FA for your personal account:

1. Go to **Profile** → **Security**
2. Click **Enable 2FA**
3. Scan QR code with authenticator app
4. Enter verification code
5. Save backup codes securely

### Session Management

View and manage active sessions:

1. Go to **Profile** → **Sessions**
2. See all active sessions
3. Click **Revoke** to end a session
4. Click **Revoke All** for all sessions except current

## Webhook Settings

Configure webhooks for real-time notifications:

### Adding a Webhook

1. Go to **Settings** → **Webhooks**
2. Click **Add Webhook**
3. Enter your endpoint URL
4. Select events to subscribe to
5. Click **Save**

### Available Events

| Event | Description |
|-------|-------------|
| `mcp.created` | New MCP added |
| `mcp.updated` | MCP settings changed |
| `mcp.deleted` | MCP removed |
| `mcp.health_changed` | MCP health status changed |
| `api_key.created` | New API key generated |
| `api_key.revoked` | API key revoked |
| `team.member_added` | New team member joined |
| `team.member_removed` | Team member removed |
| `usage.threshold` | Usage threshold reached |

### Webhook Payload

```json
{
  "event": "mcp.created",
  "timestamp": "2024-01-15T10:30:00Z",
  "organization_id": "org_123",
  "data": {
    "mcp_id": "mcp_456",
    "name": "Weather API"
  }
}
```

### Testing Webhooks

1. Click the webhook in the list
2. Click **Send Test**
3. Verify your endpoint receives the payload

## Custom Domain

*Pro and Team plans only*

Use your own domain for API calls:

### Setting Up

1. Go to **Settings** → **Custom Domain**
2. Enter your domain (e.g., `api.yourcompany.com`)
3. Add the CNAME record to your DNS:
   ```
   api.yourcompany.com CNAME gateway.plexmcp.com
   ```
4. Click **Verify Domain**
5. Wait for DNS propagation (up to 48 hours)

### Using Your Custom Domain

Once verified, use your domain for API calls:

```bash
curl -X POST https://api.yourcompany.com/mcp \
  -H "Authorization: ApiKey YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{...}'
```

## Danger Zone

Critical actions that should be used with caution:

### Delete Organization

Permanently delete your organization and all data:

1. Go to **Settings** → **Danger Zone**
2. Click **Delete Organization**
3. Type your organization name to confirm
4. Enter your password
5. Click **Delete Forever**

:::danger
This action is irreversible. All MCPs, API keys, team data, and billing history will be permanently deleted.
:::

### What Gets Deleted

- All MCP configurations
- All API keys
- Team member access
- Usage history
- Billing records (after retention period)

### Before Deleting

1. Export any data you need
2. Notify team members
3. Cancel any active subscriptions
4. Revoke API keys in use

## Audit Logs

*Team plan only*

View all actions taken in your organization:

1. Go to **Settings** → **Audit Logs**
2. Filter by date, user, or action type
3. Export logs for compliance

### Logged Actions

- Authentication events
- MCP changes
- API key operations
- Team changes
- Settings modifications
- Billing actions

### Log Retention

- Free: Not available
- Pro: 30 days
- Team: 1 year
- Enterprise: Custom retention

## Exporting Data

Export your organization data:

1. Go to **Settings** → **Data Export**
2. Select what to export:
   - MCP configurations
   - API key metadata (not secrets)
   - Team member list
   - Usage statistics
3. Click **Generate Export**
4. Download when ready

Export formats available:
- JSON
- CSV
