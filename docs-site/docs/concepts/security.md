---
sidebar_position: 2
---

# Security

PlexMCP is designed with security as a core principle. This document outlines our security practices and features.

## Security Overview

### Defense in Depth

PlexMCP implements multiple layers of security:

1. **Network Layer**: Encryption, DDoS protection, WAF
2. **Application Layer**: Authentication, authorization, validation
3. **Data Layer**: Encryption at rest, access controls, auditing
4. **Operational Layer**: Monitoring, alerting, incident response

## Authentication

### API Keys

Primary authentication mechanism:
- Cryptographically secure generation
- Hashed storage (Argon2id)
- Configurable expiration
- Instant revocation
- Per-key permissions

### Key Format

```
pmcp_01<uuid><random><signature>
│    │ │
│    │ └── Opaque payload (do not parse)
│    └─── Version number
└──────── PlexMCP prefix
```

API keys are opaque tokens. Do not attempt to parse or decode them.

### Best Practices

- Store keys in environment variables
- Rotate keys regularly (90 days recommended)
- Use minimal permissions
- Never commit keys to version control
- Monitor key usage in dashboard

## Authorization

### Role-Based Access Control

Four permission levels:

| Role | Description |
|------|-------------|
| Owner | Full access, billing, delete org |
| Admin | Manage MCPs, team, API keys |
| Member | Use MCPs, manage own keys |
| Viewer | Read-only dashboard access |

### API Key Scopes

Keys can be limited to specific MCPs:

```json
{
  "permissions": {
    "mcps": ["mcp_abc", "mcp_xyz"]
  }
}
```

## Data Protection

### Encryption in Transit

- TLS 1.3 for all connections
- HSTS enabled
- Certificate pinning for SDKs
- Perfect Forward Secrecy

### Encryption at Rest

- AES-256 for database
- Encrypted backups
- Key management via HSM
- Regular key rotation

### Data Minimization

We don't store:
- Full API keys (only hashes)
- MCP request/response content
- Tool arguments or results
- User credentials (OAuth handled by Supabase)

## Network Security

### DDoS Protection

- Anycast network distribution
- Rate limiting at edge
- Automatic traffic scrubbing
- Enterprise-grade mitigation capacity

### Web Application Firewall

- OWASP Top 10 protection
- SQL injection prevention
- XSS protection
- Bot management

### IP Restrictions

*Enterprise only*

- IP allowlisting for API access
- Geo-blocking options
- VPN/private network support

## API Security

### Rate Limiting

Protects against abuse:

| Plan | Requests/second |
|------|-----------------|
| Free | 10 |
| Pro | 100 |
| Team | 1,000 |
| Enterprise | Custom |

### Input Validation

- Schema validation on all inputs
- Size limits on payloads
- Content-type enforcement
- Request sanitization

### Error Handling

- No sensitive data in errors
- Generic error messages
- Detailed logs (internal only)
- Correlation IDs for tracking

## Audit Logging

*Team and Enterprise plans*

### What's Logged

- Authentication events
- API key operations
- MCP configuration changes
- Team membership changes
- Billing events
- Admin actions

### Log Format

```json
{
  "timestamp": "2024-01-20T15:30:00Z",
  "event": "api_key.created",
  "actor": {
    "id": "user_123",
    "email": "admin@example.com"
  },
  "resource": {
    "type": "api_key",
    "id": "key_456"
  },
  "ip": "192.168.1.1",
  "user_agent": "Mozilla/5.0..."
}
```

### Retention

- Team: 1 year
- Enterprise: Custom (up to 7 years)

## Compliance

### SOC 2 Type II

*In progress*

- Security controls audit
- Availability commitments
- Confidentiality practices
- Processing integrity

### GDPR

- EU data residency option
- Data processing agreement
- Right to erasure support
- Data portability (export)

### HIPAA

*Enterprise only*

- BAA available
- Additional security controls
- Audit requirements
- Training documentation

## Infrastructure Security

### Cloud Provider

- Major cloud providers (AWS, GCP)
- SOC 2 certified infrastructure
- Regular security assessments
- Vulnerability management

### Access Control

- Zero-trust architecture
- Multi-factor authentication
- Just-in-time access
- Privileged access management

### Monitoring

- 24/7 security monitoring
- Anomaly detection
- Automated alerting
- Incident response team

## Incident Response

### Response Process

1. **Detection**: Automated monitoring + user reports
2. **Triage**: Severity assessment within 15 minutes
3. **Containment**: Limit impact immediately
4. **Resolution**: Fix the issue
5. **Communication**: Notify affected users
6. **Review**: Post-mortem and improvements

### Notification

- Status page updates
- Email notifications
- In-app alerts (if possible)
- Post-incident report

### Contact

Report security issues:
- Email: security@plexmcp.com
- PGP key available on request
- Bug bounty program (coming soon)

## Security Features by Plan

| Feature | Free | Pro | Team | Enterprise |
|---------|------|-----|------|------------|
| API key authentication | ✓ | ✓ | ✓ | ✓ |
| TLS encryption | ✓ | ✓ | ✓ | ✓ |
| Rate limiting | ✓ | ✓ | ✓ | ✓ |
| RBAC | Basic | Full | Full | Full |
| Audit logs | - | - | ✓ | ✓ |
| SSO/SAML | - | - | ✓ | ✓ |
| IP allowlisting | - | - | - | ✓ |
| Custom retention | - | - | - | ✓ |
| BAA/HIPAA | - | - | - | ✓ |
| Dedicated support | - | - | - | ✓ |

## Recommendations

### For All Users

1. Enable MFA on your account
2. Use environment variables for keys
3. Rotate API keys regularly
4. Monitor usage in dashboard
5. Use minimal permissions

### For Team/Enterprise

1. Enable SSO/SAML
2. Review audit logs weekly
3. Set up security alerts
4. Conduct access reviews
5. Document security procedures
