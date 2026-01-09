---
sidebar_position: 1
---

# Claude Desktop Integration

Connect Claude Desktop to PlexMCP to access all your MCPs through a single gateway.

## Overview

By connecting Claude Desktop to PlexMCP, you can:
- Access multiple MCPs through one configuration
- Manage API access centrally
- Monitor usage and analytics
- Share MCPs across your team

## Prerequisites

- Claude Desktop installed
- PlexMCP account with at least one MCP registered
- An API key with access to desired MCPs

## Quick Setup

### Step 1: Get Your API Key

1. Log in to [dashboard.plexmcp.com](https://dashboard.plexmcp.com)
2. Go to **API Keys**
3. Create a new key or use an existing one
4. Copy the key

### Step 2: Configure Claude Desktop

Open your Claude Desktop configuration file:

**macOS**: `~/Library/Application Support/Claude/claude_desktop_config.json`

**Windows**: `%APPDATA%\Claude\claude_desktop_config.json`

Add the PlexMCP configuration:

```json
{
  "mcpServers": {
    "plexmcp": {
      "command": "npx",
      "args": ["-y", "@plexmcp/client"],
      "env": {
        "PLEXMCP_API_KEY": "pmcp_your_api_key_here"
      }
    }
  }
}
```

### Step 3: Restart Claude Desktop

Quit and reopen Claude Desktop to load the new configuration.

### Step 4: Verify Connection

In Claude Desktop, you should now see tools from your PlexMCP MCPs available.

## Configuration Options

### Basic Configuration

```json
{
  "mcpServers": {
    "plexmcp": {
      "command": "npx",
      "args": ["-y", "@plexmcp/client"],
      "env": {
        "PLEXMCP_API_KEY": "pmcp_xxxxx"
      }
    }
  }
}
```

### With Custom Endpoint

For self-hosted or custom domains:

```json
{
  "mcpServers": {
    "plexmcp": {
      "command": "npx",
      "args": ["-y", "@plexmcp/client"],
      "env": {
        "PLEXMCP_API_KEY": "pmcp_xxxxx",
        "PLEXMCP_API_URL": "https://api.yourcompany.com"
      }
    }
  }
}
```

### Multiple PlexMCP Accounts

Connect multiple organizations:

```json
{
  "mcpServers": {
    "plexmcp-work": {
      "command": "npx",
      "args": ["-y", "@plexmcp/client"],
      "env": {
        "PLEXMCP_API_KEY": "pmcp_work_key"
      }
    },
    "plexmcp-personal": {
      "command": "npx",
      "args": ["-y", "@plexmcp/client"],
      "env": {
        "PLEXMCP_API_KEY": "pmcp_personal_key"
      }
    }
  }
}
```

## Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `PLEXMCP_API_KEY` | Yes | Your PlexMCP API key |
| `PLEXMCP_API_URL` | No | Custom API URL |
| `PLEXMCP_TIMEOUT` | No | Request timeout (ms) |
| `PLEXMCP_DEBUG` | No | Enable debug logging |

## Using with Claude

Once configured, you can use your MCPs naturally in conversation:

**Example prompt:**
> "What's the weather in San Francisco?"

Claude will use the PlexMCP gateway to access your weather MCP.

**Example with specific tool:**
> "Use the calculator MCP to compute 15% of 250"

## Troubleshooting

### MCPs Not Appearing

1. **Check API key**: Verify key is valid and has MCP access
2. **Restart Claude**: Fully quit and reopen
3. **Check config syntax**: Ensure valid JSON
4. **Enable debug mode**: Add `"PLEXMCP_DEBUG": "true"` to env

### Connection Errors

1. **Verify network**: Ensure you can reach api.plexmcp.com
2. **Check firewall**: Allow outbound HTTPS
3. **Test API key**: Try a curl request directly

```bash
curl -X GET https://api.plexmcp.com/v1/mcps \
  -H "Authorization: ApiKey YOUR_API_KEY"
```

### Permission Denied

1. **Check key permissions**: Ensure key has access to required MCPs
2. **Verify MCP is active**: Check dashboard for MCP status
3. **Create new key**: If unsure, create a fresh key with full access

### Slow Performance

1. **Check MCP health**: Verify MCPs are responding quickly
2. **Reduce timeout**: Lower timeout if MCPs are slow
3. **Check rate limits**: Ensure you're within limits

## Updating

To update the PlexMCP client:

```bash
npx @plexmcp/client@latest --help
```

This will download the latest version.

## Logs

### macOS Logs

```bash
tail -f ~/Library/Logs/Claude/mcp*.log
```

### Debug Mode

Enable verbose logging:

```json
{
  "mcpServers": {
    "plexmcp": {
      "command": "npx",
      "args": ["-y", "@plexmcp/client"],
      "env": {
        "PLEXMCP_API_KEY": "pmcp_xxxxx",
        "PLEXMCP_DEBUG": "true"
      }
    }
  }
}
```

## Security Notes

1. **Secure your config file**: Contains your API key
2. **Use minimal permissions**: Create keys with only needed access
3. **Rotate keys regularly**: Update keys every 90 days
4. **Monitor usage**: Check dashboard for unexpected activity
