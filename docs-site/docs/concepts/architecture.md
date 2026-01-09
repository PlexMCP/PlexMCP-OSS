---
sidebar_position: 1
---

# Architecture

Understanding how PlexMCP Cloud works.

## Overview

PlexMCP is a managed gateway for Model Context Protocol (MCP) servers. It sits between your AI agents and your MCP servers, providing authentication, routing, monitoring, and management.

```
┌─────────────────────────────────────────────────────────────────┐
│                        Your AI Agents                           │
│         (Claude Desktop, Custom Apps, Integrations)             │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼ API Requests
┌─────────────────────────────────────────────────────────────────┐
│                      PlexMCP Cloud                              │
│  ┌───────────────┐  ┌───────────────┐  ┌───────────────┐       │
│  │ Authentication │  │  Rate Limiting │  │   Analytics   │       │
│  │   & Routing   │  │   & Quotas    │  │  & Monitoring │       │
│  └───────────────┘  └───────────────┘  └───────────────┘       │
│                                                                  │
│  ┌───────────────────────────────────────────────────────────┐  │
│  │                     API Gateway                            │  │
│  │  • Load Balancing     • Health Checks    • Failover       │  │
│  └───────────────────────────────────────────────────────────┘  │
└─────────────────────────────────────────────────────────────────┘
                              │
                              ▼ MCP Protocol
┌─────────────────────────────────────────────────────────────────┐
│                       Your MCP Servers                          │
│   [Weather MCP]    [Database MCP]    [Custom Tools MCP]        │
└─────────────────────────────────────────────────────────────────┘
```

## Components

### API Gateway

The core routing layer:
- Receives API requests
- Authenticates using API keys
- Routes to appropriate MCPs
- Handles responses and errors

### Authentication Service

Manages access control:
- API key validation
- Permission checking
- Rate limit enforcement
- Usage tracking

### MCP Registry

Stores MCP configurations:
- Endpoint URLs
- Authentication details
- Health status
- Tool/resource metadata

### Health Monitor

Ensures MCP availability:
- Regular health checks (60s interval)
- Status tracking
- Alerting (Pro+ plans)
- Automatic failover

### Analytics Engine

Collects and processes usage data:
- Request counts
- Latency metrics
- Error rates
- Cost tracking

## Request Flow

### 1. Authentication

```
Client Request
      │
      ▼
┌─────────────────┐
│ API Key Check   │──▶ Invalid? Return 401
└─────────────────┘
      │
      ▼ Valid
```

### 2. Authorization

```
┌─────────────────┐
│ Permission Check │──▶ No Access? Return 403
└─────────────────┘
      │
      ▼ Authorized
```

### 3. Rate Limiting

```
┌─────────────────┐
│ Rate Limit Check │──▶ Exceeded? Return 429
└─────────────────┘
      │
      ▼ Within Limits
```

### 4. Routing

```
┌─────────────────┐
│ Route to MCP    │──▶ MCP Unhealthy? Return 502
└─────────────────┘
      │
      ▼ Connected
```

### 5. Response

```
┌─────────────────┐
│ Return Response │──▶ Client
└─────────────────┘
```

## Data Storage

### What We Store

| Data Type | Storage | Purpose |
|-----------|---------|---------|
| Organization info | Database | Configuration |
| MCP configs | Database | Routing |
| API keys (hashed) | Database | Authentication |
| Usage metrics | Time-series DB | Analytics |
| Audit logs | Append-only log | Compliance |

### What We Don't Store

- Full API key secrets (only hashes)
- MCP request/response content
- Tool arguments or results
- Any PII from MCP calls

## Regional Infrastructure

:::note PlexMCP Cloud only
Multi-region infrastructure is available on PlexMCP Cloud (hosted). Self-hosted deployments run on your own infrastructure.
:::

PlexMCP Cloud runs on a global edge network:

| Region | Location | Purpose |
|--------|----------|---------|
| US-East | Virginia | Primary |
| US-West | California | Low latency |
| EU | Frankfurt | GDPR compliance |
| APAC | Singapore | Asia-Pacific |

Requests are routed to the nearest healthy region.

## High Availability

:::note PlexMCP Cloud only
SLA guarantees apply to PlexMCP Cloud only. Self-hosted uptime depends on your infrastructure.
:::

### Uptime Guarantee

- **Free**: Best effort
- **Pro**: 99.9% SLA
- **Team**: 99.9% SLA
- **Enterprise**: Up to 99.99%

### Redundancy

- Multiple availability zones
- Automatic failover
- Database replication
- CDN for static assets

### Disaster Recovery

- Daily backups
- Point-in-time recovery
- Cross-region replication
- 4-hour RTO (Enterprise)

## Security

### Network Security

- All traffic encrypted (TLS 1.3)
- DDoS protection
- WAF (Web Application Firewall)
- IP allowlisting (Enterprise)

### Application Security

- API key hashing (Argon2)
- Rate limiting
- Input validation
- Output sanitization

### Compliance

- SOC 2 Type II (in progress)
- GDPR compliant
- Data Processing Agreement available
- Regular security audits

## Integration Points

### Inbound

- REST API (`api.plexmcp.com`)
- MCP Client SDK
- Claude Desktop integration

### Outbound

- HTTP/HTTPS to your MCPs
- Webhooks for events
- Analytics exports

## Scaling

PlexMCP automatically scales based on demand:

| Metric | Scaling Behavior |
|--------|-----------------|
| Request volume | Auto-scale gateway |
| MCP count | No limits (Team+) |
| Team size | No limits (Team+) |
| Storage | Auto-expand |

## Self-Hosting

For those who need to run PlexMCP on their own infrastructure, see our [open source documentation](https://oss.plexmcp.com).

Self-hosted features:
- Full control over data
- Custom deployment options
- No usage limits
- Community support
