---
sidebar_position: 1
slug: /
---

# Welcome to PlexMCP

PlexMCP is a gateway for Model Context Protocol (MCP) servers. Connect your MCPs, manage access with API keys, and monitor usage from one dashboard.

## Why PlexMCP?

- **Single endpoint**: Route all your MCP servers through one API
- **API key management**: Create keys with scoped permissions, track usage, revoke instantly
- **Team access**: Role-based permissions for your organization
- **Usage tracking**: See what's being called, how often, and by whom
- **Self-host option**: Run it yourself with Docker or use our hosted version

## Quick Links

| I want to... | Go to... |
|--------------|----------|
| Get started in 5 minutes | [Quickstart Guide](/getting-started/quickstart) |
| Explore the dashboard | [Dashboard Tour](/getting-started/dashboard-tour) |
| Add my first MCP | [Adding Your First MCP](/getting-started/first-mcp) |
| Learn about API keys | [API Key Management](/dashboard/api-keys) |
| View API documentation | [API Reference](/api-reference/overview) |
| Self-host PlexMCP | [Open Source Docs](https://oss.plexmcp.com) |

## How It Works

```
┌─────────────────┐     ┌──────────────────┐     ┌─────────────────┐
│  Your AI Agent  │────▶│   PlexMCP Cloud  │────▶│   Your MCPs     │
│  (Claude, etc)  │     │   (Gateway)      │     │                 │
└─────────────────┘     └──────────────────┘     └─────────────────┘
        │                       │                        │
        │                       │                        │
   Uses your                Handles:               Your MCP servers
   PlexMCP API           - Authentication         (hosted anywhere)
   key                   - Rate limiting
                         - Load balancing
                         - Analytics
```

1. **Register your MCPs** in the PlexMCP dashboard
2. **Generate an API key** with the permissions you need
3. **Connect your AI agent** using the PlexMCP endpoint
4. **Monitor and scale** with real-time analytics

## Getting Started

The fastest way to get started is to [create a free account](https://dashboard.plexmcp.com/register) and follow our [5-minute quickstart guide](/getting-started/quickstart).

## Open Source

PlexMCP is open source! If you prefer to self-host, check out our [open source documentation](https://oss.plexmcp.com) for Docker, Kubernetes, and binary deployment options.

## Need Help?

- **Documentation**: You're already here!
- **GitHub Discussions**: [Community support](https://github.com/PlexMCP/PlexMCP-OSS/discussions)
- **Email Support**: support@plexmcp.com (Pro and Team plans)

## Documentation Accuracy

These docs are validated against the current implementation. If you find any mismatch between documentation and actual behavior, please [open an issue](https://github.com/PlexMCP/PlexMCP-OSS/issues) with:

- The specific endpoint or example that failed
- Expected behavior (from docs)
- Actual behavior (from server)
- Server version (if known)
