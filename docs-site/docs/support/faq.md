---
sidebar_position: 1
---

# Frequently Asked Questions

Common questions about PlexMCP.

## General

### What is PlexMCP?

PlexMCP is a unified gateway for Model Context Protocol (MCP) servers. It lets you manage, monitor, and secure access to multiple MCPs through a single dashboard and API.

### What is MCP?

Model Context Protocol (MCP) is a standard for AI agents to interact with tools and resources. MCPs expose functions (tools), data (resources), and templates (prompts) that AI assistants like Claude can use.

### Is PlexMCP open source?

Yes! PlexMCP is open source. You can self-host it or use our managed cloud service. See [oss.plexmcp.com](https://oss.plexmcp.com) for self-hosting documentation.

### How is PlexMCP different from using MCPs directly?

PlexMCP adds:
- **Unified authentication**: One API key for all MCPs
- **Management dashboard**: Visual interface for configuration
- **Analytics**: Usage tracking and monitoring
- **Team collaboration**: Multi-user access with roles
- **Security**: Rate limiting, access controls, audit logs

## Getting Started

### How do I get started?

1. [Create an account](https://dashboard.plexmcp.com/register)
2. Add your first MCP
3. Generate an API key
4. Start making requests

See our [Quickstart Guide](/getting-started/quickstart) for details.

### Do I need to install anything?

For the cloud service, no installation is needed. Just sign up and start using the API.

For self-hosting, see [oss.plexmcp.com](https://oss.plexmcp.com).

### What MCPs can I use?

Any MCP that implements the standard protocol can be registered with PlexMCP, including:
- Custom MCPs you build
- Community MCPs
- Third-party MCP services

## API & Integration

### How do I authenticate?

Use API keys in the Authorization header:

```bash
curl -H "Authorization: ApiKey pmcp_xxxxx" https://api.plexmcp.com/v1/mcps
```

### What's the API base URL?

```
https://api.plexmcp.com/v1
```

### Are there SDKs available?

Yes! Official SDKs for:
- TypeScript/JavaScript: `npm install @plexmcp/sdk`
- Python: `pip install plexmcp`
- Go: `go get github.com/PlexMCP/PlexMCP-OSS-go`

### How do I connect Claude Desktop?

Add PlexMCP to your Claude Desktop config:

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

See [Claude Desktop Integration](/guides/integrations/claude-desktop) for details.

## Pricing & Billing

### Is there a free tier?

Yes! The free tier includes:
- 5 MCPs
- 5 API connections
- 1,000 requests/month
- 1 team member

### What happens if I exceed my limits?

**Free plan**: API returns 429 errors until next billing period.

**Paid plans**: Overage charges apply:
- Pro: $0.50 per 1,000 requests
- Team: $0.25 per 1,000 requests

### Can I try Pro before paying?

Yes! Start a 14-day free trial of Pro. No credit card required.

### How do I cancel?

Go to **Billing** → **Cancel Subscription**. Access continues until period end.

## Security

### Is my data secure?

Yes. We implement:
- TLS 1.3 encryption for all traffic
- API keys hashed with Argon2
- SOC 2 compliant infrastructure
- Regular security audits

We don't store MCP request/response content.

### Can I use SSO/SAML?

SSO/SAML is available on Team and Enterprise plans.

### Do you offer audit logs?

Audit logs are available on Team and Enterprise plans, with configurable retention.

## Troubleshooting

### My MCP shows as "Unhealthy"

1. Check if your MCP server is running
2. Verify the endpoint URL is correct
3. Test the MCP directly with curl
4. Check your MCP server logs

### API returns "Invalid API Key"

1. Verify the complete key was copied
2. Check the key hasn't been revoked
3. Ensure it hasn't expired
4. Confirm you're using `ApiKey` prefix

### Requests are slow

1. Check MCP server performance
2. Verify network connectivity
3. Consider using a closer region
4. Contact support if issue persists

### Rate limited (429 errors)

1. Implement exponential backoff
2. Check `Retry-After` header
3. Consider upgrading your plan
4. Optimize request patterns

## Enterprise

### What's included in Enterprise?

- Custom request limits
- Dedicated support
- Custom SLA (up to 99.99%)
- IP allowlisting
- Extended audit log retention
- SSO/SAML
- On-premises option

### How do I get Enterprise pricing?

Contact [sales@plexmcp.com](mailto:sales@plexmcp.com) with your requirements.

### Can I self-host with Enterprise support?

Yes. Enterprise includes support for self-hosted deployments.

## Still Have Questions?

- **Documentation**: You're here!
- **Community**: [GitHub Discussions](https://github.com/PlexMCP/PlexMCP-OSS/discussions)
- **Email**: [support@plexmcp.com](mailto:support@plexmcp.com)
- **Sales**: [sales@plexmcp.com](mailto:sales@plexmcp.com)
