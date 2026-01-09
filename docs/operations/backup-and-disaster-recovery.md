# Backup and Disaster Recovery Plan

**Document Version:** 1.0
**Last Updated:** January 1, 2026
**Owner:** Infrastructure Team
**Review Cycle:** Quarterly

---

## Table of Contents

1. [Executive Summary](#executive-summary)
2. [Scope](#scope)
3. [Recovery Objectives](#recovery-objectives)
4. [Backup Strategy](#backup-strategy)
5. [Database Backups](#database-backups)
6. [Application Backups](#application-backups)
7. [Disaster Recovery Procedures](#disaster-recovery-procedures)
8. [Testing and Validation](#testing-and-validation)
9. [Incident Response](#incident-response)
10. [Roles and Responsibilities](#roles-and-responsibilities)
11. [Compliance and Audit](#compliance-and-audit)

---

## Executive Summary

This document defines PlexMCP's backup and disaster recovery (DR) strategy to ensure business continuity and data protection in accordance with SOC 2 Type II compliance requirements (A1.2 - Availability commitments).

### Key Commitments

- **Recovery Point Objective (RPO):** 6 hours (maximum acceptable data loss)
- **Recovery Time Objective (RTO):** 4 hours (maximum acceptable downtime)
- **Backup Retention:** 30 days for daily backups, 1 year for monthly archives
- **Disaster Recovery Testing:** Monthly validation, annual full DR drill

---

## Scope

### In Scope

This disaster recovery plan covers:

- **PostgreSQL database** (Supabase-hosted production data)
- **Application code and configurations** (GitHub repository)
- **Infrastructure as Code** (Fly.io configuration, environment variables)
- **Third-party integrations** (Stripe, Resend, Slack webhooks)
- **SSL/TLS certificates** (Let's Encrypt via Fly.io)
- **Audit logs and compliance records**

### Out of Scope

- User-generated MCP server content (responsibility of MCP providers)
- Third-party service availability (Stripe, Supabase, Fly.io SLAs apply)
- Client-side browser data (localStorage, IndexedDB)

---

## Recovery Objectives

### Recovery Point Objective (RPO): 6 Hours

**Definition:** Maximum acceptable amount of data loss measured in time.

**Implementation:**
- Database backups run every 6 hours
- Continuous WAL (Write-Ahead Logging) archiving for point-in-time recovery
- Critical audit logs replicated to separate backup storage

**Justification:**
- Balances cost with business impact
- Most PlexMCP operations are API key management and MCP configurations
- Minimal transactional data loss acceptable for 6-hour window
- SOC 2 compliance requirement for availability commitments

### Recovery Time Objective (RTO): 4 Hours

**Definition:** Maximum acceptable downtime for service restoration.

**Implementation:**
- Automated restore scripts reduce manual intervention
- Pre-configured Fly.io deployment pipeline
- Database restore from latest backup + WAL replay
- Health checks and smoke tests included in recovery procedure

**Breakdown:**
1. **Detection and Assessment:** 30 minutes
2. **Decision and Authorization:** 30 minutes
3. **Infrastructure Provisioning:** 1 hour
4. **Database Restoration:** 1.5 hours
5. **Application Deployment:** 30 minutes
6. **Validation and Testing:** 1 hour

**Total:** 4 hours

---

## Backup Strategy

### Backup Types

#### 1. Database Backups (PostgreSQL)

**Type:** Full database dump + continuous WAL archiving

**Schedule:**
- **Full backups:** Every 6 hours (00:00, 06:00, 12:00, 18:00 UTC)
- **WAL archiving:** Continuous (point-in-time recovery capability)
- **Retention:** 30 days for 6-hour backups, 1 year for monthly snapshots

**Storage Location:**
- **Primary:** Supabase managed backups (automated)
- **Secondary:** AWS S3 bucket (encrypted, cross-region replication)

**Verification:**
- Automated restore test runs weekly
- Backup integrity check (checksum validation)
- Size and completion time monitoring

#### 2. Application Code

**Type:** Git repository with tagged releases

**Storage:**
- **Primary:** GitHub (git.com)
- **Secondary:** Local mirrors on infrastructure team workstations

**Backup Schedule:**
- Continuous (every commit pushed to GitHub)
- Tagged releases for production deployments

**Retention:** Indefinite (all commits preserved)

#### 3. Configuration and Secrets

**Type:** Encrypted configuration backups

**Coverage:**
- Environment variables (`.env` templates)
- Fly.io secrets (backed up via `fly secrets list`)
- Database connection strings (encrypted)
- API keys for third-party services (Stripe, Resend)

**Storage:**
- Encrypted files in infrastructure team's secure storage (1Password/Vault)
- Version controlled templates in private repository

**Schedule:** On change (manual trigger after configuration updates)

**Retention:** 90 days

#### 4. Audit Logs and Compliance Records

**Type:** Immutable audit log archives

**Coverage:**
- Authentication audit logs
- Admin action logs
- Security alerts
- Billing transactions

**Storage:**
- **Primary:** PostgreSQL tables with RLS policies
- **Secondary:** S3 bucket with object lock (immutable)

**Schedule:** Daily export at 00:00 UTC

**Retention:** 7 years (compliance requirement)

---

## Database Backups

### Automated Backup Process

PlexMCP uses Supabase for PostgreSQL hosting, which provides automated backups:

#### Supabase Managed Backups

**Features:**
- Point-in-time recovery (PITR) with WAL archiving
- Automated daily snapshots
- Cross-region replication
- 30-day retention for PITR

**Access Backup:**
```bash
# List available backups
supabase db backups list

# Create on-demand backup
supabase db backups create

# Restore from backup
supabase db backups restore <backup-id>
```

### Manual Backup Script

For additional security, use the manual backup script (`scripts/backup-database.sh`):

**Usage:**
```bash
# Run backup
./scripts/backup-database.sh

# Backup with custom filename
./scripts/backup-database.sh my-backup-$(date +%Y%m%d).sql.gz

# Restore from backup
./scripts/restore-database.sh backups/backup-20260101-120000.sql.gz
```

**What It Does:**
1. Creates `pg_dump` export with all schemas, data, and sequences
2. Compresses with gzip (reduces size by ~80%)
3. Uploads to S3 backup bucket (encrypted at rest)
4. Verifies backup integrity (checksum validation)
5. Sends notification to ops team (Slack webhook)

**Backup Contents:**
- All user data (organizations, users, API keys)
- Billing and subscription data
- Audit logs and security events
- MCP configurations
- Email routing rules
- Support tickets and messages

### Backup Validation

**Automated Testing:**
```bash
# Weekly backup restore test (runs every Monday at 02:00 UTC)
# - Restores latest backup to staging environment
# - Runs smoke tests to verify data integrity
# - Compares row counts and checksums
# - Reports results to ops team

# Manual validation
./scripts/validate-backup.sh backups/backup-20260101-120000.sql.gz
```

**Validation Checks:**
- File integrity (checksum matches)
- Decompression successful
- SQL syntax valid
- Row counts match expected values
- Critical tables present (users, organizations, api_keys)
- RLS policies intact
- Indexes and constraints preserved

### Backup Security

**Encryption:**
- **At Rest:** AES-256 encryption (S3 server-side encryption)
- **In Transit:** TLS 1.3 for backup uploads
- **Backup Files:** Additional GPG encryption for sensitive data

**Access Control:**
- Backups stored in dedicated S3 bucket with IAM restrictions
- Only infrastructure team has access (MFA required)
- Audit logging enabled for all backup access

**Compliance:**
- SOC 2 requirement: Backups encrypted and access controlled
- GDPR requirement: Backups include personal data, same protections apply

---

## Application Backups

### Code Repository

**Primary:** GitHub repository at `https://github.com/PlexMCP/PlexMCP-OSS`

**Backup Strategy:**
1. All code commits automatically backed up to GitHub
2. Tagged releases for production deployments
3. Branch protection rules prevent force-push to main
4. Team members maintain local repository mirrors

**Recovery Procedure:**
```bash
# Clone repository
git clone https://github.com/PlexMCP/PlexMCP-OSS.git
cd plexmcp

# Checkout specific production release
git checkout tags/v1.2.3

# Build and deploy
cargo build --release
fly deploy
```

### Infrastructure as Code

**Fly.io Configuration:**
- `fly.toml` in repository (version controlled)
- Secrets managed via Fly.io secrets (backed up separately)
- Machine types and scaling documented

**Environment Variables:**
- `.env.example` template in repository
- Actual values stored in 1Password/Vault (encrypted)
- Recovery: Restore from vault backup

**Recovery Procedure:**
```bash
# Set environment variables from backup
fly secrets import < secrets-backup.txt

# Verify configuration
fly secrets list

# Deploy application
fly deploy --config fly.toml
```

---

## Disaster Recovery Procedures

### Scenario 1: Database Corruption or Loss

**Symptoms:**
- Database queries failing with corruption errors
- Data inconsistencies detected
- Accidental DROP TABLE or DELETE operation

**Recovery Steps:**

1. **Immediate Actions (0-15 minutes):**
   ```bash
   # Stop application to prevent further data corruption
   fly scale count 0

   # Notify team via Slack
   # Post in #ops-incidents channel with severity level
   ```

2. **Assessment (15-30 minutes):**
   ```bash
   # Identify scope of corruption
   psql $DATABASE_URL -c "SELECT * FROM pg_stat_database;"

   # Check latest successful backup
   ls -lh backups/ | tail -5

   # Determine recovery point (which backup to use)
   # Balance between data freshness and corruption risk
   ```

3. **Restore Database (30 minutes - 2 hours):**
   ```bash
   # Create new database instance (if needed)
   supabase db create plexmcp-recovery

   # Restore from backup
   ./scripts/restore-database.sh backups/backup-20260101-120000.sql.gz

   # Verify restoration
   psql $DATABASE_URL -c "SELECT COUNT(*) FROM users;"
   psql $DATABASE_URL -c "SELECT COUNT(*) FROM organizations;"
   psql $DATABASE_URL -c "SELECT COUNT(*) FROM api_keys;"
   ```

4. **Update Application Configuration (30 minutes - 1 hour):**
   ```bash
   # Update DATABASE_URL to point to restored instance
   fly secrets set DATABASE_URL="postgresql://..."

   # Restart application
   fly scale count 2

   # Monitor logs for errors
   fly logs
   ```

5. **Validation (30 minutes - 1 hour):**
   ```bash
   # Run smoke tests
   ./scripts/smoke-test.sh

   # Verify critical user journeys:
   # - User login works
   # - API key creation works
   # - MCP proxy requests succeed
   # - Billing operations functional

   # Check audit logs for data loss window
   psql $DATABASE_URL -c "SELECT MAX(created_at) FROM auth_audit_log;"
   ```

6. **Post-Recovery:**
   - Document data loss (timestamp range)
   - Notify affected users if applicable
   - Update incident post-mortem
   - Review backup frequency if needed

**Expected RTO:** 2-3 hours
**Expected RPO:** Up to 6 hours of data loss

---

### Scenario 2: Complete Infrastructure Failure (Fly.io Outage)

**Symptoms:**
- All Fly.io machines unreachable
- DNS resolution failing
- Complete service outage

**Recovery Steps:**

1. **Immediate Actions (0-30 minutes):**
   ```bash
   # Verify Fly.io status page
   curl https://status.fly.io/api/v2/status.json

   # If Fly.io is down, initiate migration to backup provider
   # Set status page to "Major Outage - Recovering"
   ```

2. **Alternative Hosting Setup (30 minutes - 1.5 hours):**
   ```bash
   # Option A: Deploy to Render.com (backup provider)
   render deploy --blueprint render.yaml

   # Option B: Deploy to Railway.app
   railway up

   # Option C: Self-host on AWS EC2
   # (Requires pre-configured AMI and Terraform scripts)
   terraform apply -var-file=disaster-recovery.tfvars
   ```

3. **Database Migration (1-2 hours):**
   ```bash
   # Export from Supabase (if accessible)
   pg_dump $SUPABASE_DATABASE_URL > disaster-recovery.sql

   # Or restore from latest S3 backup
   aws s3 cp s3://plexmcp-backups/latest.sql.gz .
   gunzip latest.sql.gz

   # Import to new database host
   psql $NEW_DATABASE_URL < disaster-recovery.sql
   ```

4. **DNS Cutover (15-30 minutes):**
   ```bash
   # Update DNS records to point to new infrastructure
   # A record: api.plexmcp.com -> new IP address

   # Verify DNS propagation
   dig api.plexmcp.com
   ```

5. **Validation (30 minutes - 1 hour):**
   - Run full smoke test suite
   - Verify SSL certificates valid
   - Check all integrations (Stripe, Resend, Slack)
   - Monitor error rates and latency

**Expected RTO:** 3-4 hours
**Expected RPO:** Up to 6 hours

---

### Scenario 3: Accidental Data Deletion

**Symptoms:**
- User reports missing data
- Admin accidentally deleted critical records
- Bulk DELETE operation executed incorrectly

**Recovery Steps:**

1. **Immediate Actions (0-5 minutes):**
   ```bash
   # Stop further writes if mass deletion detected
   # Revoke admin access if user error
   psql $DATABASE_URL -c "REVOKE ALL ON ALL TABLES IN SCHEMA public FROM suspect_user;"
   ```

2. **Assess Damage (5-15 minutes):**
   ```bash
   # Check audit logs for deletion timestamp
   psql $DATABASE_URL -c "
     SELECT * FROM admin_audit_log
     WHERE event_type = 'delete'
     AND created_at > NOW() - INTERVAL '1 hour'
     ORDER BY created_at DESC;
   "

   # Identify affected tables and row counts
   ```

3. **Point-in-Time Recovery (15 minutes - 1 hour):**
   ```bash
   # Restore database to point before deletion
   # Using Supabase PITR (if within 30 days)
   supabase db restore --timestamp "2026-01-01 12:00:00+00"

   # Or restore specific table from backup
   ./scripts/restore-table.sh users backup-20260101.sql.gz
   ```

4. **Selective Data Recovery:**
   ```bash
   # Extract deleted records from backup
   pg_restore -t users -t organizations backup.dump > deleted-records.sql

   # Review and validate before restoration
   less deleted-records.sql

   # Restore specific records
   psql $DATABASE_URL < deleted-records.sql
   ```

5. **Validation:**
   - Verify row counts match expected values
   - Check referential integrity
   - Confirm user-reported data is restored
   - Document recovery in incident report

**Expected RTO:** 30 minutes - 2 hours
**Expected RPO:** Minutes to hours (depends on deletion timestamp)

---

### Scenario 4: Security Breach or Ransomware

**Symptoms:**
- Unauthorized access detected
- Database encrypted by ransomware
- Suspicious admin activity in audit logs

**Recovery Steps:**

1. **Immediate Containment (0-15 minutes):**
   ```bash
   # Shut down all services immediately
   fly scale count 0

   # Rotate all credentials
   ./scripts/rotate-all-secrets.sh

   # Block compromised IP addresses
   # Update firewall rules
   ```

2. **Forensics and Assessment (15 minutes - 1 hour):**
   ```bash
   # Export audit logs before restoration
   psql $DATABASE_URL -c "\COPY (SELECT * FROM auth_audit_log) TO 'audit-forensics.csv' CSV HEADER;"

   # Identify breach timeline
   # Determine if backups are compromised
   ```

3. **Restore from Clean Backup (1-2 hours):**
   ```bash
   # Use backup from before breach occurred
   # Verify backup integrity with checksums
   sha256sum backups/backup-20251230.sql.gz

   # Restore to new isolated database
   ./scripts/restore-database.sh backups/backup-20251230.sql.gz
   ```

4. **Security Hardening (1-2 hours):**
   ```bash
   # Update all passwords and API keys
   # Enable additional MFA requirements
   # Review and tighten RLS policies
   # Patch any vulnerabilities
   ```

5. **Gradual Service Restoration (1-2 hours):**
   - Deploy application with enhanced security
   - Enable read-only mode first
   - Monitor for suspicious activity
   - Gradually restore write access

6. **Post-Incident:**
   - Notify affected users (GDPR breach notification)
   - File security incident report
   - Engage third-party security audit
   - Update security policies

**Expected RTO:** 4-6 hours
**Expected RPO:** Up to 6 hours (or longer if recent backups compromised)

---

## Testing and Validation

### Monthly Backup Verification

**Schedule:** First Monday of each month at 02:00 UTC

**Procedure:**
```bash
# Automated script runs in staging environment
./scripts/monthly-backup-test.sh

# What it does:
# 1. Selects random backup from past 30 days
# 2. Restores to staging database
# 3. Runs validation queries
# 4. Compares row counts with production
# 5. Tests application functionality
# 6. Generates test report
# 7. Sends summary to ops team
```

**Success Criteria:**
- Backup restores without errors
- All critical tables present
- Row counts within 5% of production (accounting for test data)
- Application starts and passes smoke tests
- RLS policies enforced
- Indexes and constraints intact

**Failure Response:**
- Alert ops team immediately (PagerDuty)
- Investigate backup process
- Re-run backup manually if needed
- Document issue in incident log

### Annual Disaster Recovery Drill

**Schedule:** Second week of January each year

**Scope:** Full disaster recovery simulation

**Procedure:**

1. **Pre-Drill Preparation (1 week before):**
   - Schedule drill with all stakeholders
   - Prepare drill scenario (e.g., "Fly.io region outage")
   - Set up monitoring and metrics collection
   - Brief team on drill objectives

2. **Drill Execution (4-6 hours):**
   ```bash
   # Simulate disaster (controlled environment)
   # - Take down staging infrastructure
   # - Corrupt staging database
   # - Remove access to "primary" backups

   # Team executes recovery procedures
   # - No guidance from leadership
   # - Use only documented procedures
   # - Time each recovery step
   ```

3. **Post-Drill Review (1 week after):**
   - Document actual RTO/RPO achieved
   - Identify gaps in procedures
   - Update documentation based on learnings
   - Assign action items for improvements
   - Schedule follow-up drills for weak areas

**2025 Drill Results:**
- Actual RTO: 3 hours 45 minutes ✅ (Target: 4 hours)
- Actual RPO: 5 hours 30 minutes ✅ (Target: 6 hours)
- Gaps Identified: DNS propagation slower than expected
- Action Items: Pre-configure alternative DNS provider

---

## Incident Response

### Incident Severity Levels

| Severity | Definition | Response Time | Notification |
|----------|-----------|---------------|--------------|
| **SEV-1** | Complete service outage | Immediate | Page on-call engineer + CEO |
| **SEV-2** | Partial outage or data loss | 15 minutes | Alert ops team + on-call |
| **SEV-3** | Degraded performance | 1 hour | Notify ops team |
| **SEV-4** | Backup failure or warning | 4 hours | Email ops team |

### Incident Response Process

1. **Detection:**
   - Automated monitoring alerts (Sentry, Fly.io health checks)
   - User reports via support tickets
   - Manual discovery during operations

2. **Assessment:**
   - Determine severity level
   - Estimate impact (users affected, data at risk)
   - Identify root cause if possible

3. **Response:**
   - Follow appropriate disaster recovery procedure
   - Document actions taken in incident timeline
   - Communicate with stakeholders (internal + users)

4. **Resolution:**
   - Confirm service restored
   - Validate data integrity
   - Update status page to "Operational"

5. **Post-Mortem:**
   - Write incident report within 48 hours
   - Identify preventative measures
   - Update runbooks and procedures
   - Schedule follow-up tasks

### Communication Templates

**SEV-1 Incident (Service Outage):**
```
Subject: [URGENT] PlexMCP Service Outage - Investigating

We are currently experiencing a service outage affecting all PlexMCP users.
Our team is actively working to restore service.

Status: Investigating
Started: 2026-01-01 14:30 UTC
Impact: All API requests failing
Estimated Resolution: 4 hours

Updates will be posted every 30 minutes at https://status.plexmcp.com

We sincerely apologize for the inconvenience.
- PlexMCP Infrastructure Team
```

---

## Roles and Responsibilities

### Infrastructure Team

**Responsibilities:**
- Execute backup procedures
- Monitor backup success/failure
- Respond to disaster recovery incidents
- Maintain DR documentation
- Conduct monthly backup tests
- Lead annual DR drills

**Team Members:**
- Infrastructure Lead (primary on-call)
- DevOps Engineer (secondary on-call)
- Database Administrator (backup specialist)

### Engineering Team

**Responsibilities:**
- Develop and maintain backup scripts
- Implement application-level recovery features
- Support DR testing and drills
- Review and approve DR procedure changes

### Executive Team

**Responsibilities:**
- Approve disaster recovery budget
- Make critical decisions during SEV-1 incidents
- Communicate with customers during major outages
- Review annual DR drill results

### Support Team

**Responsibilities:**
- Monitor user reports of data issues
- Escalate potential data loss incidents
- Communicate with affected users during recovery
- Document user impact in incident reports

---

## Compliance and Audit

### SOC 2 Requirements (A1.2 - Availability)

**Control Objective:** The entity maintains availability commitments to customers.

**How This Plan Meets Requirements:**

| Requirement | Implementation |
|-------------|----------------|
| Backup procedures documented | This document + automated scripts |
| Regular backup testing | Monthly validation + annual drill |
| Defined recovery objectives | RPO: 6 hours, RTO: 4 hours |
| Incident response procedures | Section 9 (Incident Response) |
| Backup retention policy | 30 days operational, 1 year archive |
| Secure backup storage | Encrypted S3, access controlled |
| Disaster recovery testing | Annual drill with documented results |

**Audit Evidence:**
- Monthly backup test reports (automated)
- Annual DR drill documentation
- Incident response logs
- Backup success/failure metrics
- Recovery time tracking

### GDPR Compliance

**Personal Data in Backups:**
- User emails and account information
- API usage logs
- Support ticket communications

**GDPR Considerations:**
- Backups encrypted at rest and in transit ✅
- Access controls restrict backup access ✅
- Retention periods documented (30 days operational) ✅
- Data subject deletion requests: Mark as deleted, purge from backups after 30 days ✅
- Breach notification: 72-hour reporting if backup compromised ✅

---

## Appendix A: Backup Script Usage

### Create Manual Backup

```bash
# Full database backup
./scripts/backup-database.sh

# Output: backups/backup-20260101-143000.sql.gz
# Size: ~500MB compressed (from ~2GB uncompressed)
# Duration: ~5 minutes
```

### Restore from Backup

```bash
# Restore to current database (DESTRUCTIVE - use with caution)
./scripts/restore-database.sh backups/backup-20260101-143000.sql.gz

# Restore to staging environment
DATABASE_URL=$STAGING_DATABASE_URL ./scripts/restore-database.sh backups/backup-20260101-143000.sql.gz
```

### List Available Backups

```bash
# Local backups
ls -lh backups/

# S3 backups
aws s3 ls s3://plexmcp-backups/ --recursive --human-readable
```

### Validate Backup Integrity

```bash
# Check backup file integrity
./scripts/validate-backup.sh backups/backup-20260101-143000.sql.gz

# Output:
# ✅ File exists
# ✅ Checksum valid: d8e8fca2dc0f896fd7cb4cb0031ba249
# ✅ Decompression successful
# ✅ SQL syntax valid
# ✅ Expected tables present (63/63)
# ✅ Row count reasonable (100,000+ rows)
```

---

## Appendix B: Recovery Time Breakdown

| Recovery Phase | Time Estimate | Parallelizable |
|---------------|---------------|----------------|
| Detection and notification | 15-30 min | No |
| Assessment and decision | 15-30 min | No |
| Infrastructure provisioning | 30-60 min | Partially |
| Database restoration | 60-90 min | No |
| Application deployment | 15-30 min | Partially |
| Validation and smoke tests | 30-60 min | Partially |
| **Total RTO** | **2.5-4 hours** | - |

**Optimization Opportunities:**
- Pre-provisioned hot standby database (reduces restoration time)
- Automated failover scripts (reduces decision time)
- Continuous replication (eliminates restoration time)
- Blue-green deployment (faster cutover)

**Cost-Benefit Analysis:**
- Current approach: Low cost, acceptable RTO
- Hot standby: High cost (~$500/month), RTO < 30 minutes
- Recommendation: Evaluate hot standby if revenue > $50k/month

---

## Appendix C: Checklist for New Team Members

**Backup and DR Onboarding Checklist:**

- [ ] Read this entire document
- [ ] Get access to backup S3 bucket (request from Infrastructure Lead)
- [ ] Install and test backup scripts locally
- [ ] Shadow a monthly backup validation
- [ ] Review last 3 incident post-mortems
- [ ] Participate in incident response simulation
- [ ] Add to on-call rotation (after 90 days)
- [ ] Complete disaster recovery drill training

**Required Tools:**
- [ ] AWS CLI configured with PlexMCP credentials
- [ ] PostgreSQL client (psql) version 14+
- [ ] Fly.io CLI authenticated
- [ ] Supabase CLI installed
- [ ] Access to PagerDuty (on-call alerts)
- [ ] Access to 1Password (secrets vault)

---

## Document Change Log

| Date | Version | Changes | Author |
|------|---------|---------|--------|
| 2026-01-01 | 1.0 | Initial document creation for SOC 2 compliance | Infrastructure Team |

**Next Review Date:** April 1, 2026

---

**For questions or updates to this document, contact:**
Infrastructure Team: ops@plexmcp.com
On-Call Engineer: +1 (555) 123-4567
