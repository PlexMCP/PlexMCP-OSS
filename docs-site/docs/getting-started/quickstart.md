---
sidebar_position: 1
---

# Quickstart Guide

Get up and running with PlexMCP in 5 minutes. This guide will walk you through creating an account, adding your first MCP, and generating an API key.

## Prerequisites

- An MCP server you want to connect (or use our test MCP)
- A modern web browser

## Step 1: Create Your Account

1. Go to [dashboard.plexmcp.com/register](https://dashboard.plexmcp.com/register)
2. Sign up with your email or continue with Google/GitHub
3. Verify your email address (if using email signup)
4. You'll be redirected to your new dashboard

## Step 2: Create an Organization

When you first sign in, you'll be prompted to create an organization:

1. Enter your organization name (e.g., "My Company" or "Personal")
2. Choose a unique slug (e.g., "my-company") - this will be used in API calls
3. Click **Create Organization**

## Step 3: Add Your First MCP

1. Navigate to **MCPs** in the sidebar
2. Click **Add MCP**
3. Fill in the MCP details:
   - **Name**: A friendly name (e.g., "Weather API")
   - **Endpoint URL**: Your MCP server URL (e.g., `https://my-mcp.example.com`)
   - **Description**: Optional description for your team
4. Click **Create MCP**

:::tip Getting Started
Don't have an MCP ready? Check out our [SDK examples](/guides/integrations/sdk-examples) to see how to build and connect your first MCP server.
:::

## Step 4: Generate an API Key

1. Navigate to **API Keys** in the sidebar
2. Click **Create API Key**
3. Configure your key:
   - **Name**: A descriptive name (e.g., "Development Key")
   - **Expiration**: Choose when the key expires (or never)
   - **Permissions**: Select which MCPs this key can access
4. Click **Create**
5. **Copy your API key immediately** - you won't be able to see it again!

## Step 5: Make Your First Request

Use your API key to call an MCP through PlexMCP:

```bash
curl -X POST https://api.plexmcp.com/mcp \
  -H "Authorization: ApiKey YOUR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{
    "mcp_id": "your-mcp-id",
    "tool": "your-tool-name",
    "arguments": {}
  }'
```

See [MCP API Reference](/api-reference/mcps#invoke-mcp-tool) for full request/response details.

Or configure Claude Desktop to use PlexMCP:

```json
{
  "mcpServers": {
    "plexmcp": {
      "command": "npx",
      "args": ["-y", "@plexmcp/client"],
      "env": {
        "PLEXMCP_API_KEY": "YOUR_API_KEY"
      }
    }
  }
}
```

## Next Steps

- [Dashboard Tour](/getting-started/dashboard-tour) - Learn about all dashboard features
- [Adding MCPs](/getting-started/first-mcp) - Deep dive into MCP configuration
- [API Key Best Practices](/guides/authentication/api-keys) - Secure your API keys
- [Claude Desktop Integration](/guides/integrations/claude-desktop) - Connect Claude Desktop

## Troubleshooting

### "Invalid API Key" Error
- Make sure you copied the entire API key
- Check that the key hasn't expired
- Verify the key has permission to access the requested MCP

### "MCP Not Found" Error
- Verify the MCP ID in your request
- Check that your API key has permission for this MCP
- Ensure the MCP is active in your dashboard

### Connection Timeouts
- Check that your MCP server is running and accessible
- Verify the endpoint URL is correct
- Check for any firewall rules blocking the connection
