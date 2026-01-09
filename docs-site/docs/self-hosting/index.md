---
sidebar_position: 1
---

# Self-Hosting PlexMCP

PlexMCP is open source and can be self-hosted on your own infrastructure. This gives you complete control over your data and deployment.

## Why Self-Host?

- **Data Control**: Keep all your MCP data on your own servers
- **Privacy**: No data leaves your infrastructure
- **Customization**: Modify the code to fit your needs
- **Compliance**: Meet regulatory requirements for data residency
- **Cost**: No per-seat or usage-based pricing

## Quick Start

Get PlexMCP running in under 5 minutes:

```bash
# Clone the repository
git clone https://github.com/PlexMCP/PlexMCP-OSS.git
cd plexmcp

# Run the setup script
./scripts/setup.sh

# Start all services
docker compose up -d

# Open in browser
open http://localhost:3000
```

## Deployment Options

### Docker Compose (Recommended)

Best for: Small to medium deployments, single-server setups.

- Includes PostgreSQL, Redis, API, and Web frontend
- Easy to set up and maintain
- Suitable for most self-hosting scenarios

[Docker Deployment Guide →](./docker.md)

### Manual Deployment

Best for: Large deployments, existing infrastructure, custom requirements.

- Deploy components individually
- Integrate with existing databases and services
- Full control over each component

[Manual Deployment Guide →](./manual.md)

### Kubernetes

Best for: Large-scale production deployments.

- Helm charts available (coming soon)
- Horizontal scaling support
- High availability configuration

## What's Included

When you self-host PlexMCP, you get:

| Feature | Included |
|---------|----------|
| MCP Server Management | ✅ |
| MCP Proxy/Gateway | ✅ |
| API Key Management | ✅ |
| Organization & Teams | ✅ |
| Two-Factor Authentication | ✅ |
| Usage Analytics | ✅ |
| Rate Limiting | ✅ |
| Row-Level Security | ✅ |

### Not Included in Self-Hosted

The following features are only available on PlexMCP Cloud:

- Stripe billing integration
- Admin dashboard
- Support ticket system
- Custom domain SSL auto-provisioning
- Multi-region deployment

## System Requirements

See [Requirements](./requirements.md) for detailed specifications.

**Minimum:**
- 2 CPU cores
- 4GB RAM
- 20GB storage
- Docker 24+ or Podman 4+

**Recommended:**
- 4 CPU cores
- 8GB RAM
- 100GB SSD storage

## Getting Help

- **Documentation**: You're here!
- **GitHub Issues**: [Report bugs](https://github.com/PlexMCP/PlexMCP-OSS/issues)
- **GitHub Discussions**: [Ask questions](https://github.com/PlexMCP/PlexMCP-OSS/discussions)
- **Discord**: [Join our community](https://discord.gg/HAYYTGnht8)

## License

PlexMCP is licensed under [FSL-1.1-Apache-2.0](https://github.com/PlexMCP/PlexMCP-OSS/blob/main/LICENSE).

- **Free for**: Individuals, businesses under $1M revenue, self-hosting
- **Commercial license required for**: Businesses with $1M+ revenue
- **Converts to Apache 2.0**: January 2031

See [Commercial License](https://github.com/PlexMCP/PlexMCP-OSS/blob/main/COMMERCIAL_LICENSE.md) for details.
