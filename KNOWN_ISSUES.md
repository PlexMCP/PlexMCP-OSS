# PlexMCP Known Issues & Solutions

**Purpose**: Document recurring issues and their solutions to prevent repeated debugging efforts.

**Last Updated**: 2025-12-27

---

## Critical Deployment Issues

### 🚨 GitHub → Vercel Auto-Deploy Not Working

**Issue**: Code pushed to GitHub `main` branch does not automatically deploy to Vercel.

**Symptoms**:
- Git commits pushed successfully
- No deployment activity in Vercel dashboard
- Production site shows old code after hard refresh
- Version indicators/debug code don't appear

**Root Cause**: Vercel GitHub integration webhook not configured or disabled

**Solution**:
```bash
# Manual deployment via Vercel CLI
cd ./web
npx vercel --prod --yes

# Verify deployment
vercel domains ls
vercel inspect <deployment-url> --logs
```

**Prevention**:
1. Check Vercel → Project Settings → Git Integration
2. Verify GitHub webhook is active: https://github.com/PlexMCP/PlexMCP-OSS/settings/hooks
3. Test auto-deploy by pushing a small change to a test file
4. Add deployment status badge to README (optional)

**Related Debugging Session**: `DEBUG_staff_emails_dropdown.md` (Attempts 1-6)

---

## Deployment Infrastructure

### Frontend Deployments (Vercel)

**Primary Domain**: `dashboard.plexmcp.com`
**Vercel Project**: `web` (under `axon-hub` team)
**Git Branch**: `main`
**Build Command**: `npm run build`
**Output Directory**: `.next`

**All Configured Domains**:
- `plexmcp.com` → web project
- `www.plexmcp.com` → web project
- `dashboard.plexmcp.com` → web project
- `docs.plexmcp.com` → plexmcp-cloud-docs project
- `cdocs.plexmcp.com` → plexmcp-cloud-docs project
- `oss.plexmcp.com` → docs-site project

**Deployment Verification**:
```bash
# Check domain configuration
vercel domains inspect plexmcp.com

# View recent deployments
vercel ls

# Test production URL
curl -I https://dashboard.plexmcp.com
```

### Backend Deployments (Fly.io)

**Primary Domain**: `plexmcp-api.fly.dev`
**Fly App**: `plexmcp-api`
**Machines**:
- API machine: `2873d37c67e498`
- Worker machine: `2874234fd70728`

**Manual Deployment**:
```bash
fly deploy --app plexmcp-api
fly status --app plexmcp-api
fly logs --app plexmcp-api
```

---

## Database Issues

### Issue: Column Doesn't Exist Errors

**Example**: `ERROR: column "full_name" does not exist`

**Symptoms**:
- Backend returns 500 errors
- SQL queries fail in logs
- API endpoints crash

**Root Cause**: SQL queries reference columns that were removed or never existed

**Solution**:
1. Check actual database schema:
   ```bash
   PGPASSWORD="YOUR_DATABASE_PASSWORD" psql -h db.yjstfwfdmybeaadauell.supabase.co -U postgres -d postgres -c "\d table_name"
   ```
2. Update SQL queries to match actual schema
3. Use SQLx compile-time verification: `cargo sqlx prepare`

**Prevention**:
- Always verify column names against production schema before writing queries
- Use SQLx offline mode for compile-time SQL verification
- Document schema changes in migration files

---

## Frontend/Backend Integration Issues

### Issue: API Response Structure Mismatch

**Example**: Backend returns `{ users: [...] }` but frontend expects `{ items: [...] }`

**Symptoms**:
- API call succeeds (200 OK)
- Data appears in Network tab
- Frontend shows "No data" or empty arrays
- Console shows `undefined` when accessing properties

**Solution**:
1. Check backend response structure:
   ```bash
   curl -H "Authorization: Bearer <token>" https://plexmcp-api.fly.dev/api/v1/endpoint | jq
   ```

2. Use existing transformation hooks:
   ```typescript
   // Instead of custom useQuery:
   import { useAdminUsers } from "@/lib/api/hooks/use-admin";
   const { data } = useAdminUsers(page, limit);
   // Hook handles transformation automatically
   ```

3. If custom query needed, transform in queryFn:
   ```typescript
   queryFn: async () => {
     const res = await apiClient.endpoint();
     return {
       items: res.data.users || [],  // Transform backend format
       total: res.data.total,
     };
   }
   ```

**Prevention**:
- Prefer using existing API hooks from `src/lib/api/hooks/`
- Document response structures in TypeScript interfaces
- Add response examples in API documentation

---

## Debugging Best Practices

### Add Visible Deployment Indicators

When debugging deployment issues, add **visible** indicators to confirm code updates:

```typescript
// Add version badge to page
<Badge variant="outline">v{Date.now()}</Badge>

// Add debug info box
<div className="bg-yellow-50 p-2 text-xs">
  Debug: version={version} | data={data?.length}
</div>
```

**Why**: Console logs can be cached or filtered. Visual indicators immediately confirm deployment.

### Progressive Debugging Steps

1. **Verify deployment updated**:
   - Check version indicator appears
   - Check Network tab for new bundle hash
   - Hard refresh with cache clear (Cmd+Shift+R)

2. **Check API requests**:
   - Network tab → Filter by "Fetch/XHR"
   - Verify endpoint being called
   - Check response status and payload

3. **Inspect data flow**:
   ```typescript
   console.log('1. Raw API response:', response);
   console.log('2. Extracted data:', response.data);
   console.log('3. Transformed:', transformedData);
   console.log('4. Filtered:', filteredData);
   ```

4. **Test in isolation**:
   - Test API endpoint directly with curl
   - Test React Query hook in isolation
   - Test filtering logic with sample data

---

## Common Error Patterns

### React Query Not Running

**Symptoms**:
- No network requests in Network tab
- `data` is `undefined`
- Component renders but no API call made

**Possible Causes**:
1. `enabled: false` condition
2. Missing dependencies in dependency array
3. Query key doesn't change when it should
4. React Query cache returning stale data

**Solutions**:
```typescript
// Check enabled condition
const { data } = useQuery({
  queryKey: [...],
  queryFn: ...,
  enabled: !!requiredValue,  // ← Check this
});

// Force refetch
queryClient.invalidateQueries({ queryKey: [...] });

// Disable cache for debugging
const { data } = useQuery({
  queryKey: [...],
  queryFn: ...,
  gcTime: 0,  // Don't cache
});
```

### Vercel Build Failures

**Check build logs**:
```bash
vercel inspect <deployment-url> --logs
```

**Common causes**:
- TypeScript errors (run `npm run build` locally first)
- Missing environment variables
- Dependency installation failures
- Memory limit exceeded

**Solutions**:
1. Test build locally: `npm run build`
2. Check `.env.production` variables are set in Vercel
3. Increase Node memory: `NODE_OPTIONS=--max-old-space-size=4096`

---

## Maintenance Checklist

### Before Deploying to Production

- [ ] All tests pass locally (`npm test`, `cargo test`)
- [ ] Build succeeds locally (`npm run build`, `cargo build --release`)
- [ ] Environment variables set in Vercel/Fly.io
- [ ] Database migrations tested on staging
- [ ] No TODO/FIXME comments in critical code
- [ ] Debug code removed (console.logs, test data)

### After Deployment

- [ ] Hard refresh production URL
- [ ] Check Network tab for 500 errors
- [ ] Verify key user flows work
- [ ] Check error monitoring (Sentry/logs)
- [ ] Test on mobile if UI changed

### When Debugging Deployment Issues

- [ ] Verify git commit pushed to correct branch
- [ ] Check Vercel dashboard for deployment activity
- [ ] Inspect deployment logs for errors
- [ ] Test manual deployment if auto-deploy fails
- [ ] Document issue in this file if novel

---

## Quick Reference Commands

### Verify Production State

```bash
# Frontend - Check deployed version
curl -I https://dashboard.plexmcp.com | grep -i "x-vercel"

# Backend - Check health
curl https://plexmcp-api.fly.dev/api/v1/health

# Database - Check connection
PGPASSWORD="YOUR_DATABASE_PASSWORD" psql -h db.yjstfwfdmybeaadauell.supabase.co -U postgres -d postgres -c "SELECT version();"
```

### Force Deployment

```bash
# Frontend
cd ./web
npx vercel --prod --yes

# Backend
fly deploy --app plexmcp-api

# Verify both
curl -I https://dashboard.plexmcp.com
curl https://plexmcp-api.fly.dev/api/v1/health
```

### Debug Database Issues

```bash
# List all tables
PGPASSWORD="YOUR_DATABASE_PASSWORD" psql -h db.yjstfwfdmybeaadauell.supabase.co -U postgres -d postgres -c "\dt"

# Describe table schema
PGPASSWORD="YOUR_DATABASE_PASSWORD" psql -h db.yjstfwfdmybeaadauell.supabase.co -U postgres -d postgres -c "\d table_name"

# Run test query
PGPASSWORD="YOUR_DATABASE_PASSWORD" psql -h db.yjstfwfdmybeaadauell.supabase.co -U postgres -d postgres -c "SELECT * FROM users LIMIT 1;"
```

---

## Issue Tracking

When you encounter a new issue:

1. **Document in this file** under appropriate section
2. **Create debug log** in `/DEBUG_<feature>_<date>.md` for complex issues
3. **Add to git**: `git add KNOWN_ISSUES.md DEBUG_*.md`
4. **Reference in commit**: Link debug log in commit message

**Example**:
```bash
git add KNOWN_ISSUES.md DEBUG_staff_emails_dropdown.md
git commit -m "docs: document auto-deploy issue and staff emails fix

See DEBUG_staff_emails_dropdown.md for full debugging session.
Root cause: Vercel auto-deploy webhook not configured."
```

---

*This document should be updated whenever a new class of issues is discovered or solved.*
