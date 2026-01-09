<p align="center">
  <a href="https://plexmcp.com">
    <img src="web/public/logo-purple-512.png" alt="PlexMCP Logo" width="80">
  </a>
</p>

<h1 align="center">PlexMCP</h1>

<p align="center">
  The MCP gateway platform.
  <br />
  <a href="https://oss.plexmcp.com"><strong>Docs</strong></a> · <a href="https://plexmcp.com"><strong>PlexMCP Cloud</strong></a> · <a href="https://discord.gg/HAYYTGnht8"><strong>Discord</strong></a>
</p>

<p align="center">
  <a href="https://github.com/PlexMCP/PlexMCP-OSS/blob/main/LICENSE"><img src="https://img.shields.io/badge/License-FSL--1.1--Apache--2.0-blue.svg" alt="License"></a>
  <a href="https://github.com/PlexMCP/PlexMCP-OSS/actions/workflows/docker-publish.yml"><img src="https://github.com/PlexMCP/PlexMCP-OSS/actions/workflows/docker-publish.yml/badge.svg" alt="Build"></a>
  <a href="https://github.com/PlexMCP/PlexMCP-OSS/releases"><img src="https://img.shields.io/github/v/release/PlexMCP/PlexMCP-OSS" alt="Release"></a>
  <a href="https://discord.gg/HAYYTGnht8"><img src="https://img.shields.io/badge/Discord-Join%20Server-5865F2?logo=discord&logoColor=white" alt="Discord"></a>
</p>

---

PlexMCP is a unified gateway for managing and orchestrating [MCP (Model Context Protocol)](https://modelcontextprotocol.io) servers. It provides enterprise-grade authentication, multi-tenant isolation, and comprehensive audit logging for AI applications.

- **MCP Server Orchestration** with connection pooling and routing ([Docs](https://oss.plexmcp.com/concepts/architecture))
- **Multi-Tenant Organizations** with complete data isolation ([Docs](https://oss.plexmcp.com/dashboard/team))
- **API Key Management** with scoped permissions and rate limiting ([Docs](https://oss.plexmcp.com/dashboard/api-keys))
- **Two-Factor Authentication** with TOTP support ([Docs](https://oss.plexmcp.com/concepts/security))
- **Usage Analytics** for monitoring and billing ([Docs](https://oss.plexmcp.com/dashboard/billing))
- **Audit Logging** for compliance and debugging ([Docs](https://oss.plexmcp.com/concepts/security))

## Getting Started

### Self-Hosting with Docker

```bash
# Clone the repository
git clone https://github.com/PlexMCP/PlexMCP-OSS.git
cd PlexMCP-OSS

# Run setup (generates .env with secrets)
./scripts/setup.sh

# Start with pre-built images
docker compose --profile prebuilt up -d

# Open http://localhost:3000
```

> For detailed instructions, see the [Self-Hosting Guide](https://oss.plexmcp.com/self-hosting).

### PlexMCP Cloud

The fastest way to get started is with [PlexMCP Cloud](https://plexmcp.com) - our managed platform with:

- Instant setup, no infrastructure to manage
- Automatic SSL and custom domains
- Usage-based billing with free tier
- Priority support and SLA

[Get started for free](https://dashboard.plexmcp.com/register)

## Documentation

**Docs are verified against implementation.** See [`/docs-site`](./docs-site) and [Documentation Accuracy](https://oss.plexmcp.com/intro#documentation-accuracy).

- [Getting Started](https://oss.plexmcp.com/getting-started/quickstart) - Quick start guide
- [Self-Hosting](https://oss.plexmcp.com/self-hosting) - Deploy on your infrastructure
- [API Reference](https://oss.plexmcp.com/api-reference/overview) - REST API documentation
- [Security](https://oss.plexmcp.com/concepts/security) - Security architecture

## Architecture

PlexMCP is built with:

- **Backend**: [Rust](https://www.rust-lang.org/) with [Axum](https://github.com/tokio-rs/axum) web framework
- **Frontend**: [Next.js](https://nextjs.org/) 15 with TypeScript
- **Database**: PostgreSQL 15+ with [SQLx](https://github.com/launchbadge/sqlx)
- **Cache**: Redis 7+

```
PlexMCP-OSS/
├── crates/
│   ├── api/        # API server
│   ├── billing/    # Billing integration
│   ├── shared/     # Shared types
│   └── worker/     # Background jobs
├── web/            # Next.js dashboard
├── migrations/     # Database migrations
└── docs-site/      # Documentation
```

## Community & Support

- [Discord](https://discord.gg/HAYYTGnht8) - Chat with the community
- [GitHub Issues](https://github.com/PlexMCP/PlexMCP-OSS/issues) - Bug reports and feature requests
- [GitHub Discussions](https://github.com/PlexMCP/PlexMCP-OSS/discussions) - Questions and ideas
- [Documentation](https://oss.plexmcp.com) - Guides and reference

For enterprise support, contact [support@plexmcp.com](mailto:support@plexmcp.com).

## Contributing

We welcome contributions! See our [Contributing Guide](CONTRIBUTING.md) for details.

```bash
# Development setup
cargo install cargo-watch sqlx-cli
cargo watch -x run

# Run tests
cargo test --workspace
```

## Security

Security is a top priority. PlexMCP includes:

- Row-Level Security (RLS) on all database tables
- Encryption at rest and in transit
- SOC 2 Type II compliance ready
- OWASP Top 10 protections

For security issues, please email [security@plexmcp.com](mailto:security@plexmcp.com). See our [Security Policy](SECURITY.md).

## License

PlexMCP is source-available under the [FSL-1.1-Apache-2.0](LICENSE) license:

- **Self-host freely** on your own infrastructure
- **Modify the source** for your needs
- **Commercial use** permitted
- **Converts to Apache 2.0** after five years

See [COMMERCIAL_LICENSE.md](COMMERCIAL_LICENSE.md) for enterprise licensing.

---

<p align="center">
  <a href="https://plexmcp.com">Website</a> · <a href="https://oss.plexmcp.com">Docs</a> · <a href="https://discord.gg/HAYYTGnht8">Discord</a> · <a href="https://twitter.com/plexmcp">Twitter</a>
</p>
