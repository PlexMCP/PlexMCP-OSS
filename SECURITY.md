# Security Policy

## Supported Versions

We actively support the following versions with security updates:

| Version | Supported          | Status |
| ------- | ------------------ | ------ |
| 1.0.x   | :white_check_mark: | Current stable release |
| < 1.0   | :x:                | No longer supported |

We recommend always running the latest stable release to ensure you have the latest security patches.

---

## Reporting a Vulnerability

**Please do not report security vulnerabilities through public GitHub issues.**

We take the security of PlexMCP seriously. If you discover a security vulnerability, please follow these steps:

### 1. Email Security Team

Send details to: **security@plexmcp.com**

### What to Include in Your Report

To help us triage and fix the issue as quickly as possible, please include:

1. **Description of the vulnerability**
   - Clear explanation of the security issue
   - Type of vulnerability (e.g., XSS, SQL injection, authentication bypass)

2. **Steps to reproduce**
   - Detailed step-by-step instructions
   - Include any proof-of-concept code or curl commands
   - Screenshots or videos if applicable

3. **Impact assessment**
   - What data or systems are at risk?
   - What could an attacker do with this vulnerability?
   - CVSS score estimate (if known)

4. **Affected components**
   - Specific files, endpoints, or features affected
   - Version(s) where the vulnerability exists

5. **Your contact information**
   - Name (or handle if you prefer anonymity)
   - Email address for follow-up
   - Whether you'd like to be credited in our security advisories

### What to Expect

1. **Acknowledgment:** We will acknowledge receipt of your report within **24 hours**.

2. **Initial Assessment:** We will provide an initial assessment within **72 hours**, including:
   - Validation of the vulnerability
   - Severity classification (Critical, High, Medium, Low)
   - Estimated timeline for a fix

3. **Regular Updates:** We will keep you informed of our progress:
   - Weekly updates for Critical/High severity issues
   - Bi-weekly updates for Medium/Low severity issues

4. **Resolution:** Once a fix is developed:
   - We will share the patch with you for verification before public release
   - We will coordinate the disclosure timeline with you
   - We will credit you in our security advisory (if desired)

### Our Commitment

- **No Legal Action:** We will not pursue legal action against security researchers who:
  - Report vulnerabilities in good faith
  - Avoid privacy violations, data destruction, or service interruption
  - Give us reasonable time to fix the issue before public disclosure

- **Recognition:** We will publicly credit security researchers who:
  - Responsibly disclose vulnerabilities
  - Allow us to fix the issue before public disclosure
  - Wish to be acknowledged (anonymity is also respected)

### Disclosure Timeline

We follow **coordinated disclosure** principles:

1. **Day 0:** Vulnerability reported
2. **Day 1-3:** Initial validation and assessment
3. **Day 4-30:** Develop and test fix
4. **Day 30-90:** Deploy fix to production and prepare advisory
5. **Day 90+:** Public disclosure (or earlier if fix is deployed and you agree)

For **Critical vulnerabilities** actively being exploited, we may expedite this timeline.

For **Low severity** issues, we may extend the timeline up to 180 days.

## Security Measures

PlexMCP implements the following security controls:

### Application Security

- **Authentication:** JWT-based authentication with refresh tokens
- **Two-Factor Authentication (2FA):** TOTP and backup codes supported
- **Password Security:** Argon2id hashing with salt, 12+ character minimum
- **Session Management:** Token-based sessions with configurable expiration
- **API Key Security:** HMAC-based API keys with rate limiting

### Data Security

- **Encryption at Rest:** AES-256 encryption for sensitive data
- **Encryption in Transit:** TLS 1.3 with strong cipher suites
- **Database Security:** Row-level security (RLS) policies with FORCE enforcement
- **Audit Logging:** Immutable audit logs for all sensitive operations
- **Data Isolation:** Multi-tenant architecture with strict organization boundaries

### Infrastructure Security

- **Environment Isolation:** Separate production, staging, and development environments
- **Secrets Management:** Environment variables, no hardcoded credentials
- **Backup Security:** Encrypted backups with 30-day retention
- **Monitoring:** Real-time security alerting for anomalies
- **Rate Limiting:** Request throttling to prevent abuse

### Compliance

- **SOC 2 Type II:** In progress (target: Q2 2026)
- **GDPR:** Data protection and privacy controls implemented
- **OWASP Top 10:** Mitigations for all Top 10 vulnerabilities
- **Security Audits:** Annual third-party penetration testing

## Scope

### In Scope

The following are within scope for vulnerability reports:

- **API Endpoints:** All `/api/v1/*` endpoints
- **Authentication:** Login, registration, password reset, 2FA
- **Authorization:** Role-based access control, RLS policies
- **MCP Proxy:** `/mcp` endpoint and request handling
- **Admin Panel:** Superadmin and admin functionality
- **Billing:** Stripe integration and payment processing
- **Database:** SQL injection, data leakage, privilege escalation

### Out of Scope

The following are **NOT** considered security vulnerabilities:

- **Rate Limiting Bypass:** Unless it leads to DoS or resource exhaustion
- **Missing Security Headers:** Unless exploitable (we have HSTS, CSP, etc.)
- **Self-XSS:** Requires user to paste malicious code into their own console
- **Social Engineering:** Phishing, pretexting, or physical security
- **Third-Party Services:** Issues in Stripe, Supabase, Fly.io (report to them directly)
- **Denial of Service:** Unless it requires minimal resources to trigger
- **Version Disclosure:** Knowing our software versions doesn't enable an attack
- **Open Redirect:** Unless it leads to SSRF or credential theft

### Testing Guidelines

If you wish to test for vulnerabilities, please:

✅ **DO:**
- Test against your own PlexMCP organization/account
- Use test/demo accounts only
- Stay within your organization's data boundaries
- Report findings privately before public disclosure

❌ **DO NOT:**
- Access other users' data or organizations
- Perform DoS or load testing against production
- Exploit vulnerabilities for personal gain
- Publicly disclose vulnerabilities before we've had time to fix them
- Spam or send excessive requests

## Security Advisories

We publish security advisories at:

- **GitHub Security Advisories:** https://github.com/PlexMCP/PlexMCP-OSS/security/advisories
- **Website:** https://plexmcp.com/security/advisories
- **Email List:** Subscribe at security-announce@plexmcp.com

## Bug Bounty Program

**Status:** Not currently available

We are grateful for security researchers who report vulnerabilities responsibly. While we do not currently offer monetary rewards, we do provide:

- Public recognition in our security advisories (if desired)
- A place in our Hall of Fame: https://plexmcp.com/security/hall-of-fame
- Swag and merchandise for high-impact discoveries

We may introduce a bug bounty program in the future as we grow.

## Security Team

Our security team can be reached at:

- **General Inquiries:** security@plexmcp.com
- **Urgent Issues:** Email security@plexmcp.com with [URGENT] in subject line
- **PGP Encrypted:** Use our [PGP key](https://plexmcp.com/.well-known/pgp-key.asc) for sensitive reports

## Hall of Fame

We thank the following security researchers for responsibly disclosing vulnerabilities:

*List to be populated as vulnerabilities are reported and fixed*

## Additional Resources

- [Security Documentation](docs/security/)
- [SOC 2 Compliance Information](docs/security/soc2-compliance.md)
- [Encryption Documentation](docs/security/encryption.md)
- [Incident Response Plan](docs/security/incident-response.md)
- [Access Control Procedures](docs/security/access-control.md)

## Questions?

If you have questions about our security policy or practices, please contact us at security@plexmcp.com.

---

**Last Updated:** January 1, 2026
**Version:** 1.0
