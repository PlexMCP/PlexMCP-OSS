//! MCP Method Handlers
//!
//! Handles individual MCP methods like initialize, tools/list, tools/call, etc.

use std::sync::Arc;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};
use serde_json::Value;
use sqlx::PgPool;
use uuid::Uuid;

use super::client::McpClient;
use super::router::{McpMethod, McpRouter};
use super::types::*;
use crate::routes::mcp_proxy::McpFilter;

/// MCP Proxy Handler
///
/// Orchestrates handling of MCP requests by aggregating results from
/// multiple upstream MCPs.
pub struct McpProxyHandler {
    client: Arc<McpClient>,
    router: McpRouter,
    pool: PgPool,
    config: Arc<crate::config::Config>,
}

/// Upstream MCP configuration loaded from database
#[derive(Debug, Clone)]
pub struct UpstreamMcp {
    pub id: Uuid,
    pub name: String,
    pub transport: McpTransport,
    pub is_active: bool,
    pub request_timeout_ms: i32,
    pub partial_timeout_ms: Option<i32>,
}

/// Response wrapper that includes MCP tracking metadata for analytics
///
/// This struct captures which MCPs were accessed during request processing,
/// enabling accurate per-MCP usage tracking and analytics.
#[derive(Debug)]
pub struct McpTrackedResponse {
    /// The JSON-RPC response to return to the client
    pub response: JsonRpcResponse,
    /// List of MCP instance IDs that were accessed during this request
    ///
    /// - **Single-target methods** (tools/call, resources/read, prompts/get): Contains exactly one ID
    /// - **Aggregation methods** (tools/list, resources/list, prompts/list): Contains all queried MCP IDs
    /// - **Errors before MCP selection**: Empty vec (tracks as NULL mcp_instance_id)
    pub accessed_mcp_ids: Vec<Uuid>,
}

impl McpTrackedResponse {
    /// Create a tracked response with no MCPs accessed
    ///
    /// Use for:
    /// - `initialize` requests (no MCP interaction)
    /// - Errors that occur before MCP selection (auth failures, parse errors)
    /// - Notifications that don't require responses
    pub fn without_mcps(response: JsonRpcResponse) -> Self {
        Self {
            response,
            accessed_mcp_ids: vec![],
        }
    }

    /// Create a tracked response for a single-target MCP request
    ///
    /// Use for methods that route to exactly one MCP:
    /// - `tools/call` - calls a specific tool on one MCP
    /// - `resources/read` - reads from one MCP
    /// - `prompts/get` - gets a prompt from one MCP
    pub fn with_single_mcp(response: JsonRpcResponse, mcp_id: Uuid) -> Self {
        Self {
            response,
            accessed_mcp_ids: vec![mcp_id],
        }
    }

    /// Create a tracked response for aggregation requests
    ///
    /// Use for methods that query multiple MCPs:
    /// - `tools/list` - aggregates tools from all accessible MCPs
    /// - `resources/list` - aggregates resources from all accessible MCPs
    /// - `prompts/list` - aggregates prompts from all accessible MCPs
    ///
    /// Creates one usage record per MCP for accurate analytics breakdown.
    pub fn with_mcps(response: JsonRpcResponse, mcp_ids: Vec<Uuid>) -> Self {
        Self {
            response,
            accessed_mcp_ids: mcp_ids,
        }
    }
}

impl McpProxyHandler {
    /// Create a new proxy handler with shared MCP client
    pub fn new(
        pool: PgPool,
        config: Arc<crate::config::Config>,
        mcp_client: Arc<McpClient>,
    ) -> Self {
        Self {
            client: mcp_client,
            router: McpRouter::new(),
            pool,
            config,
        }
    }

    /// Helper to safely create a success response with JSON serialization error handling
    fn success_response<T: serde::Serialize>(id: Option<JsonRpcId>, value: &T) -> JsonRpcResponse {
        match serde_json::to_value(value) {
            Ok(v) => JsonRpcResponse::success(id, v),
            Err(e) => {
                tracing::error!("Failed to serialize response: {}", e);
                JsonRpcResponse::error(
                    id,
                    JsonRpcError {
                        code: -32603, // Internal error
                        message: format!("Serialization error: {}", e),
                        data: None,
                    },
                )
            }
        }
    }

    /// Load active MCPs for an organization
    pub async fn load_mcps(&self, org_id: Uuid) -> Result<Vec<UpstreamMcp>, sqlx::Error> {
        self.load_mcps_filtered(org_id, None).await
    }

    /// Load active MCPs for an organization with optional filtering
    pub async fn load_mcps_filtered(
        &self,
        org_id: Uuid,
        filter: Option<&McpFilter>,
    ) -> Result<Vec<UpstreamMcp>, sqlx::Error> {
        #[derive(sqlx::FromRow)]
        struct McpRow {
            id: Uuid,
            name: String,
            mcp_type: String,
            config: Value,
            status: String,
            request_timeout_ms: i32,
            partial_timeout_ms: Option<i32>,
        }

        let rows: Vec<McpRow> = sqlx::query_as(
            r#"
            SELECT id, name, mcp_type, config, status, request_timeout_ms, partial_timeout_ms
            FROM mcp_instances
            WHERE org_id = $1 AND status = 'active'
            ORDER BY name
            "#,
        )
        .bind(org_id)
        .fetch_all(&self.pool)
        .await?;

        let mcps: Vec<UpstreamMcp> = rows
            .into_iter()
            .filter_map(|row| {
                let config = row.config;
                let transport = self.parse_transport(&row.mcp_type, config)?;
                Some(UpstreamMcp {
                    id: row.id,
                    name: row.name,
                    transport,
                    is_active: row.status == "active",
                    request_timeout_ms: row.request_timeout_ms,
                    partial_timeout_ms: row.partial_timeout_ms,
                })
            })
            // Apply MCP access filtering based on API key settings
            .filter(|mcp| {
                match filter {
                    Some(f) if f.mode == "selected" => {
                        // Only include MCPs in the allowed list
                        if let Some(allowed_ids) = &f.allowed_ids {
                            allowed_ids.contains(&mcp.id)
                        } else {
                            // No allowed IDs means nothing is accessible
                            false
                        }
                    }
                    Some(f) if f.mode == "none" => {
                        // No MCPs accessible (should be caught earlier, but just in case)
                        false
                    }
                    _ => {
                        // "all" mode or no filter - include everything
                        true
                    }
                }
            })
            .collect();

        Ok(mcps)
    }

    /// Parse transport configuration from database
    fn parse_transport(&self, mcp_type: &str, config: Value) -> Option<McpTransport> {
        let endpoint_url = config.get("endpoint_url")?.as_str()?.to_string();

        // Parse authentication
        let auth = self.parse_auth(&config);

        match mcp_type {
            "http" => Some(McpTransport::Http { endpoint_url, auth }),
            "sse" | "websocket" => Some(McpTransport::Sse { endpoint_url, auth }),
            "stdio" => {
                let command = config.get("command")?.as_str()?.to_string();
                let args = config
                    .get("args")
                    .and_then(|a| a.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                let env = config
                    .get("env")
                    .and_then(|e| e.as_object())
                    .map(|obj| {
                        obj.iter()
                            .filter_map(|(k, v)| Some((k.clone(), v.as_str()?.to_string())))
                            .collect()
                    })
                    .unwrap_or_default();
                Some(McpTransport::Stdio { command, args, env })
            }
            _ => {
                // Default to HTTP if type is unknown but we have an endpoint
                Some(McpTransport::Http { endpoint_url, auth })
            }
        }
    }

    /// Parse authentication from config
    fn parse_auth(&self, config: &Value) -> McpAuth {
        let auth_type = config
            .get("auth_type")
            .and_then(|v| v.as_str())
            .unwrap_or("none");

        match auth_type {
            "bearer" => {
                if let Some(token) = config.get("api_key").and_then(|v| v.as_str()) {
                    McpAuth::Bearer {
                        token: token.to_string(),
                    }
                } else {
                    McpAuth::None
                }
            }
            "api-key" | "api_key" => {
                let header = config
                    .get("api_key_header")
                    .and_then(|v| v.as_str())
                    .unwrap_or("X-API-Key")
                    .to_string();
                if let Some(value) = config.get("api_key").and_then(|v| v.as_str()) {
                    McpAuth::ApiKey {
                        header,
                        value: value.to_string(),
                    }
                } else {
                    McpAuth::None
                }
            }
            "basic" => {
                let username = config
                    .get("username")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                let password = config
                    .get("password")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                McpAuth::Basic { username, password }
            }
            _ => McpAuth::None,
        }
    }

    /// Handle an incoming MCP request
    pub async fn handle_request(
        &self,
        org_id: Uuid,
        request: JsonRpcRequest,
    ) -> McpTrackedResponse {
        // Delegate to filtered handler with no filter (all MCPs accessible)
        self.handle_request_filtered(
            org_id,
            request,
            McpFilter {
                mode: "all".to_string(),
                allowed_ids: None,
            },
        )
        .await
    }

    /// Handle an incoming MCP request with MCP access filtering
    ///
    /// Returns a tracked response that includes which MCPs were accessed,
    /// enabling accurate per-MCP usage tracking for analytics.
    pub async fn handle_request_filtered(
        &self,
        org_id: Uuid,
        request: JsonRpcRequest,
        filter: McpFilter,
    ) -> McpTrackedResponse {
        let method = McpRouter::get_method_type(&request.method);

        match method {
            McpMethod::Initialize => self.handle_initialize(request.id).await,
            McpMethod::Notification => {
                // Notifications don't get responses, no MCP interaction
                McpTrackedResponse::without_mcps(JsonRpcResponse::success(None, Value::Null))
            }
            McpMethod::ToolsList => {
                self.handle_tools_list_filtered(org_id, request.id, &filter)
                    .await
            }
            McpMethod::ToolsCall => {
                self.handle_tools_call_filtered(org_id, request.id, request.params, &filter)
                    .await
            }
            McpMethod::ResourcesList => {
                self.handle_resources_list_filtered(org_id, request.id, &filter)
                    .await
            }
            McpMethod::ResourcesRead => {
                self.handle_resources_read_filtered(org_id, request.id, request.params, &filter)
                    .await
            }
            McpMethod::PromptsList => {
                self.handle_prompts_list_filtered(org_id, request.id, &filter)
                    .await
            }
            McpMethod::PromptsGet => {
                self.handle_prompts_get_filtered(org_id, request.id, request.params, &filter)
                    .await
            }
            _ => McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                request.id,
                JsonRpcError::method_not_found(&request.method),
            )),
        }
    }

    /// Handle initialize request
    ///
    /// Returns server capabilities. No MCPs are accessed during initialization.
    async fn handle_initialize(&self, id: Option<JsonRpcId>) -> McpTrackedResponse {
        let result = InitializeResult {
            protocol_version: "2024-11-05".to_string(),
            capabilities: Capabilities {
                tools: Some(ToolsCapability::default()),
                resources: Some(ResourcesCapability::default()),
                prompts: Some(PromptsCapability::default()),
                logging: Some(LoggingCapability::default()),
                ..Default::default()
            },
            server_info: ServerInfo {
                name: "PlexMCP".to_string(),
                version: env!("CARGO_PKG_VERSION").to_string(),
            },
            instructions: Some(
                "PlexMCP aggregates multiple MCP servers. Tools are prefixed with their source MCP name (e.g., github:create_issue).".to_string()
            ),
        };

        McpTrackedResponse::without_mcps(Self::success_response(id, &result))
    }

    /// Handle tools/list - aggregate tools from all MCPs
    #[allow(dead_code)] // Reserved for direct MCP protocol use
    async fn handle_tools_list(&self, org_id: Uuid, id: Option<JsonRpcId>) -> McpTrackedResponse {
        self.handle_tools_list_filtered(
            org_id,
            id,
            &McpFilter {
                mode: "all".to_string(),
                allowed_ids: None,
            },
        )
        .await
    }

    /// Handle tools/list with MCP filtering
    ///
    /// Aggregates tools from all accessible MCPs. Creates one usage record
    /// per MCP for accurate analytics breakdown.
    async fn handle_tools_list_filtered(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
        filter: &McpFilter,
    ) -> McpTrackedResponse {
        let mcps = match self.load_mcps_filtered(org_id, Some(filter)).await {
            Ok(m) => m,
            Err(e) => {
                // Error before MCP selection - track with empty MCP list
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to load MCPs: {}", e)),
                ));
            }
        };

        // Capture MCP IDs for analytics tracking (before any processing)
        let accessed_mcp_ids: Vec<Uuid> = mcps.iter().map(|m| m.id).collect();

        if mcps.is_empty() {
            // Return empty list if no MCPs configured - no MCPs to track
            let result = AggregatedToolsListResult {
                tools: vec![],
                errors: vec![],
                next_cursor: None,
            };
            return McpTrackedResponse::with_mcps(
                Self::success_response(id, &result),
                vec![], // No MCPs accessed
            );
        }

        // Fetch tools from all MCPs with partial timeout
        let mut tasks = FuturesUnordered::new();

        for mcp in mcps.iter() {
            // Use per-MCP partial timeout if set, otherwise global default
            let timeout_ms = mcp
                .partial_timeout_ms
                .map(|t| t as u64)
                .unwrap_or(self.config.mcp_partial_timeout_ms);

            let client = self.client.clone();
            let transport = mcp.transport.clone();
            let mcp_name = mcp.name.clone();
            let mcp_id_uuid = mcp.id;
            let mcp_id = mcp.id.to_string();

            tasks.push(async move {
                let start = std::time::Instant::now();

                // Apply timeout to individual MCP request with circuit breaker
                let result = tokio::time::timeout(
                    Duration::from_millis(timeout_ms),
                    client.get_tools_with_breaker(mcp_id_uuid, &transport, &mcp_id),
                )
                .await;

                let elapsed = start.elapsed();

                match result {
                    Ok(Ok(tools)) => {
                        tracing::debug!(
                            mcp = %mcp_name,
                            tools_count = tools.len(),
                            latency_ms = elapsed.as_millis(),
                            "Tools retrieved successfully"
                        );
                        Ok((mcp_name, tools))
                    }
                    Ok(Err(e)) => {
                        tracing::error!(
                            mcp = %mcp_name,
                            error = %e,
                            "MCP error"
                        );
                        Err(McpError {
                            mcp_name: mcp_name.clone(),
                            error: e.to_string(),
                        })
                    }
                    Err(_) => {
                        tracing::warn!(
                            mcp = %mcp_name,
                            timeout_ms = timeout_ms,
                            "MCP timeout"
                        );
                        Err(McpError {
                            mcp_name: mcp_name.clone(),
                            error: format!("Timeout after {}ms", timeout_ms),
                        })
                    }
                }
            });
        }

        // Collect results as they complete
        let mut all_tools = Vec::new();
        let mut errors = Vec::new();

        while let Some(result) = tasks.next().await {
            match result {
                Ok((mcp_name, tools)) => {
                    // Prefix tools with MCP name
                    let prefixed = self.router.prefix_tools(&mcp_name, tools);
                    all_tools.extend(prefixed);
                }
                Err(e) => {
                    errors.push(e);
                }
            }
        }

        let result = AggregatedToolsListResult {
            tools: all_tools,
            errors,
            next_cursor: None,
        };

        // Return with all MCP IDs that were queried (for per-MCP usage tracking)
        McpTrackedResponse::with_mcps(Self::success_response(id, &result), accessed_mcp_ids)
    }

    /// Handle tools/call - route to specific MCP
    #[allow(dead_code)] // Reserved for direct MCP protocol use
    async fn handle_tools_call(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
        params: Option<Value>,
    ) -> McpTrackedResponse {
        self.handle_tools_call_filtered(
            org_id,
            id,
            params,
            &McpFilter {
                mode: "all".to_string(),
                allowed_ids: None,
            },
        )
        .await
    }

    /// Handle tools/call with MCP filtering
    ///
    /// Routes the tool call to a specific MCP. Tracks the single MCP that
    /// was accessed for accurate analytics.
    async fn handle_tools_call_filtered(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
        params: Option<Value>,
        filter: &McpFilter,
    ) -> McpTrackedResponse {
        let params: ToolCallParams = match params {
            Some(p) => match serde_json::from_value(p) {
                Ok(parsed) => parsed,
                Err(e) => {
                    // Error before MCP selection
                    return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                        id,
                        JsonRpcError::invalid_params(format!("Invalid params: {}", e)),
                    ));
                }
            },
            None => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params("Missing params"),
                ));
            }
        };

        // Parse the prefixed tool name
        let parsed = match self.router.parse_tool_name(&params.name) {
            Some(p) => p,
            None => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params(format!(
                        "Invalid tool name format. Expected 'mcp_name:tool_name', got: {}",
                        params.name
                    )),
                ));
            }
        };

        // Load MCPs and find the target (with filtering)
        let mcps = match self.load_mcps_filtered(org_id, Some(filter)).await {
            Ok(m) => m,
            Err(e) => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to load MCPs: {}", e)),
                ));
            }
        };

        let target_mcp = mcps.iter().find(|m| m.name == parsed.mcp_name);
        let mcp = match target_mcp {
            Some(m) => m,
            None => {
                // MCP not found - no MCP to track
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params(format!(
                        "MCP not found or access denied: {}",
                        parsed.mcp_name
                    )),
                ));
            }
        };

        // Capture MCP ID for analytics tracking (before the call)
        let mcp_id = mcp.id;

        // Call the tool on the upstream MCP
        let result = self
            .client
            .call_tool(
                &mcp.transport,
                &mcp.id.to_string(),
                &parsed.tool_name,
                params.arguments,
            )
            .await;

        match result {
            Ok(tool_result) => {
                // Success - track the single MCP that was called
                McpTrackedResponse::with_single_mcp(
                    Self::success_response(id, &tool_result),
                    mcp_id,
                )
            }
            Err(e) => {
                // Error during tool call - still track the MCP (it was accessed)
                McpTrackedResponse::with_single_mcp(
                    JsonRpcResponse::error(
                        id,
                        JsonRpcError::internal_error(format!(
                            "Tool call failed on {}: {}",
                            parsed.mcp_name, e
                        )),
                    ),
                    mcp_id,
                )
            }
        }
    }

    /// Handle resources/list - aggregate resources from all MCPs
    #[allow(dead_code)] // Reserved for direct MCP protocol use
    async fn handle_resources_list(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
    ) -> McpTrackedResponse {
        self.handle_resources_list_filtered(
            org_id,
            id,
            &McpFilter {
                mode: "all".to_string(),
                allowed_ids: None,
            },
        )
        .await
    }

    /// Handle resources/list with MCP filtering
    ///
    /// Aggregates resources from all accessible MCPs. Creates one usage record
    /// per MCP for accurate analytics breakdown.
    async fn handle_resources_list_filtered(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
        filter: &McpFilter,
    ) -> McpTrackedResponse {
        let mcps = match self.load_mcps_filtered(org_id, Some(filter)).await {
            Ok(m) => m,
            Err(e) => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to load MCPs: {}", e)),
                ));
            }
        };

        // Capture MCP IDs for analytics tracking (before any processing)
        let accessed_mcp_ids: Vec<Uuid> = mcps.iter().map(|m| m.id).collect();

        if mcps.is_empty() {
            let result = AggregatedResourcesListResult {
                resources: vec![],
                errors: vec![],
                next_cursor: None,
            };
            return McpTrackedResponse::with_mcps(
                Self::success_response(id, &result),
                vec![], // No MCPs accessed
            );
        }

        // Fetch resources from all MCPs in parallel
        // Fetch resources from all MCPs with partial timeout
        let mut tasks = FuturesUnordered::new();

        for mcp in mcps.iter() {
            let timeout_ms = mcp
                .partial_timeout_ms
                .map(|t| t as u64)
                .unwrap_or(self.config.mcp_partial_timeout_ms);

            let client = self.client.clone();
            let transport = mcp.transport.clone();
            let mcp_name = mcp.name.clone();
            let mcp_id_uuid = mcp.id;
            let mcp_id = mcp.id.to_string();

            tasks.push(async move {
                let start = std::time::Instant::now();

                let result = tokio::time::timeout(
                    Duration::from_millis(timeout_ms),
                    client.get_resources_with_breaker(mcp_id_uuid, &transport, &mcp_id),
                )
                .await;

                let elapsed = start.elapsed();

                match result {
                    Ok(Ok(resources)) => {
                        tracing::debug!(
                            mcp = %mcp_name,
                            resources_count = resources.len(),
                            latency_ms = elapsed.as_millis(),
                            "Resources retrieved successfully"
                        );
                        Ok((mcp_name, resources))
                    }
                    Ok(Err(e)) => {
                        tracing::error!(mcp = %mcp_name, error = %e, "MCP error");
                        Err(McpError {
                            mcp_name: mcp_name.clone(),
                            error: e.to_string(),
                        })
                    }
                    Err(_) => {
                        tracing::warn!(mcp = %mcp_name, timeout_ms = timeout_ms, "MCP timeout");
                        Err(McpError {
                            mcp_name: mcp_name.clone(),
                            error: format!("Timeout after {}ms", timeout_ms),
                        })
                    }
                }
            });
        }

        let mut all_resources = Vec::new();
        let mut errors = Vec::new();

        while let Some(result) = tasks.next().await {
            match result {
                Ok((mcp_name, resources)) => {
                    let prefixed = self.router.prefix_resources(&mcp_name, resources);
                    all_resources.extend(prefixed);
                }
                Err(e) => {
                    errors.push(e);
                }
            }
        }

        let result = AggregatedResourcesListResult {
            resources: all_resources,
            errors,
            next_cursor: None,
        };

        // Return with all MCP IDs that were queried (for per-MCP usage tracking)
        McpTrackedResponse::with_mcps(Self::success_response(id, &result), accessed_mcp_ids)
    }

    /// Handle resources/read - route to specific MCP
    #[allow(dead_code)] // Reserved for direct MCP protocol use
    async fn handle_resources_read(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
        params: Option<Value>,
    ) -> McpTrackedResponse {
        self.handle_resources_read_filtered(
            org_id,
            id,
            params,
            &McpFilter {
                mode: "all".to_string(),
                allowed_ids: None,
            },
        )
        .await
    }

    /// Handle resources/read with MCP filtering
    ///
    /// Routes the resource read to a specific MCP. Tracks the single MCP that
    /// was accessed for accurate analytics.
    async fn handle_resources_read_filtered(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
        params: Option<Value>,
        filter: &McpFilter,
    ) -> McpTrackedResponse {
        let params: ResourceReadParams = match params {
            Some(p) => match serde_json::from_value(p) {
                Ok(parsed) => parsed,
                Err(e) => {
                    // Error before MCP selection
                    return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                        id,
                        JsonRpcError::invalid_params(format!("Invalid params: {}", e)),
                    ));
                }
            },
            None => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params("Missing params"),
                ));
            }
        };

        // Parse the prefixed URI
        let parsed = match self.router.parse_resource_uri(&params.uri) {
            Some(p) => p,
            None => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params(format!(
                        "Invalid resource URI format. Expected 'plexmcp://mcp_name/uri', got: {}",
                        params.uri
                    )),
                ));
            }
        };

        // Load MCPs and find the target (with filtering)
        let mcps = match self.load_mcps_filtered(org_id, Some(filter)).await {
            Ok(m) => m,
            Err(e) => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to load MCPs: {}", e)),
                ));
            }
        };

        let target_mcp = mcps.iter().find(|m| m.name == parsed.mcp_name);
        let mcp = match target_mcp {
            Some(m) => m,
            None => {
                // MCP not found - no MCP to track
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params(format!(
                        "MCP not found or access denied: {}",
                        parsed.mcp_name
                    )),
                ));
            }
        };

        // Capture MCP ID for analytics tracking (before the read)
        let mcp_id = mcp.id;

        // Read the resource from the upstream MCP
        let result = self
            .client
            .read_resource(&mcp.transport, &mcp.id.to_string(), &parsed.original_uri)
            .await;

        match result {
            Ok(read_result) => {
                // Success - track the single MCP that was accessed
                McpTrackedResponse::with_single_mcp(
                    Self::success_response(id, &read_result),
                    mcp_id,
                )
            }
            Err(e) => {
                // Error during read - still track the MCP (it was accessed)
                McpTrackedResponse::with_single_mcp(
                    JsonRpcResponse::error(
                        id,
                        JsonRpcError::internal_error(format!(
                            "Resource read failed on {}: {}",
                            parsed.mcp_name, e
                        )),
                    ),
                    mcp_id,
                )
            }
        }
    }

    /// Handle prompts/list - aggregate prompts from all MCPs
    #[allow(dead_code)] // Reserved for direct MCP protocol use
    async fn handle_prompts_list(&self, org_id: Uuid, id: Option<JsonRpcId>) -> McpTrackedResponse {
        self.handle_prompts_list_filtered(
            org_id,
            id,
            &McpFilter {
                mode: "all".to_string(),
                allowed_ids: None,
            },
        )
        .await
    }

    /// Handle prompts/list with MCP filtering
    ///
    /// Aggregates prompts from all accessible MCPs. Creates one usage record
    /// per MCP for accurate analytics breakdown.
    async fn handle_prompts_list_filtered(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
        filter: &McpFilter,
    ) -> McpTrackedResponse {
        let mcps = match self.load_mcps_filtered(org_id, Some(filter)).await {
            Ok(m) => m,
            Err(e) => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to load MCPs: {}", e)),
                ));
            }
        };

        // Capture MCP IDs for analytics tracking (before any processing)
        let accessed_mcp_ids: Vec<Uuid> = mcps.iter().map(|m| m.id).collect();

        if mcps.is_empty() {
            let result = AggregatedPromptsListResult {
                prompts: vec![],
                errors: vec![],
                next_cursor: None,
            };
            return McpTrackedResponse::with_mcps(
                Self::success_response(id, &result),
                vec![], // No MCPs accessed
            );
        }

        // Fetch prompts from all MCPs with partial timeout
        let mut tasks = FuturesUnordered::new();

        for mcp in mcps.iter() {
            let timeout_ms = mcp
                .partial_timeout_ms
                .map(|t| t as u64)
                .unwrap_or(self.config.mcp_partial_timeout_ms);

            let client = self.client.clone();
            let transport = mcp.transport.clone();
            let mcp_name = mcp.name.clone();
            let mcp_id_uuid = mcp.id;
            let mcp_id = mcp.id.to_string();

            tasks.push(async move {
                let start = std::time::Instant::now();

                let result = tokio::time::timeout(
                    Duration::from_millis(timeout_ms),
                    client.get_prompts_with_breaker(mcp_id_uuid, &transport, &mcp_id),
                )
                .await;

                let elapsed = start.elapsed();

                match result {
                    Ok(Ok(prompts)) => {
                        tracing::debug!(
                            mcp = %mcp_name,
                            prompts_count = prompts.len(),
                            latency_ms = elapsed.as_millis(),
                            "Prompts retrieved successfully"
                        );
                        Ok((mcp_name, prompts))
                    }
                    Ok(Err(e)) => {
                        tracing::error!(mcp = %mcp_name, error = %e, "MCP error");
                        Err(McpError {
                            mcp_name: mcp_name.clone(),
                            error: e.to_string(),
                        })
                    }
                    Err(_) => {
                        tracing::warn!(mcp = %mcp_name, timeout_ms = timeout_ms, "MCP timeout");
                        Err(McpError {
                            mcp_name: mcp_name.clone(),
                            error: format!("Timeout after {}ms", timeout_ms),
                        })
                    }
                }
            });
        }

        let mut all_prompts = Vec::new();
        let mut errors = Vec::new();

        while let Some(result) = tasks.next().await {
            match result {
                Ok((mcp_name, prompts)) => {
                    let prefixed = self.router.prefix_prompts(&mcp_name, prompts);
                    all_prompts.extend(prefixed);
                }
                Err(e) => {
                    errors.push(e);
                }
            }
        }

        let result = AggregatedPromptsListResult {
            prompts: all_prompts,
            errors,
            next_cursor: None,
        };

        // Return with all MCP IDs that were queried (for per-MCP usage tracking)
        McpTrackedResponse::with_mcps(Self::success_response(id, &result), accessed_mcp_ids)
    }

    /// Handle prompts/get - route to specific MCP
    #[allow(dead_code)] // Reserved for direct MCP protocol use
    async fn handle_prompts_get(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
        params: Option<Value>,
    ) -> McpTrackedResponse {
        self.handle_prompts_get_filtered(
            org_id,
            id,
            params,
            &McpFilter {
                mode: "all".to_string(),
                allowed_ids: None,
            },
        )
        .await
    }

    /// Handle prompts/get with MCP filtering
    ///
    /// Routes the prompt request to a specific MCP. Tracks the single MCP that
    /// was accessed for accurate analytics.
    async fn handle_prompts_get_filtered(
        &self,
        org_id: Uuid,
        id: Option<JsonRpcId>,
        params: Option<Value>,
        filter: &McpFilter,
    ) -> McpTrackedResponse {
        let params: PromptGetParams = match params {
            Some(p) => match serde_json::from_value(p) {
                Ok(parsed) => parsed,
                Err(e) => {
                    // Error before MCP selection
                    return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                        id,
                        JsonRpcError::invalid_params(format!("Invalid params: {}", e)),
                    ));
                }
            },
            None => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params("Missing params"),
                ));
            }
        };

        // Parse the prefixed prompt name (same format as tools: mcp_name:prompt_name)
        let parsed = match self.router.parse_tool_name(&params.name) {
            Some(p) => p,
            None => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params(format!(
                        "Invalid prompt name format. Expected 'mcp_name:prompt_name', got: {}",
                        params.name
                    )),
                ));
            }
        };

        // Load MCPs and find the target (with filtering)
        let mcps = match self.load_mcps_filtered(org_id, Some(filter)).await {
            Ok(m) => m,
            Err(e) => {
                // Error before MCP selection
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::internal_error(format!("Failed to load MCPs: {}", e)),
                ));
            }
        };

        let target_mcp = mcps.iter().find(|m| m.name == parsed.mcp_name);
        let mcp = match target_mcp {
            Some(m) => m,
            None => {
                // MCP not found - no MCP to track
                return McpTrackedResponse::without_mcps(JsonRpcResponse::error(
                    id,
                    JsonRpcError::invalid_params(format!(
                        "MCP not found or access denied: {}",
                        parsed.mcp_name
                    )),
                ));
            }
        };

        // Capture MCP ID for analytics tracking (before the call)
        let mcp_id = mcp.id;

        // Call the upstream MCP with the original prompt name
        match self
            .client
            .get_prompt(
                &mcp.transport,
                &mcp.id.to_string(),
                &parsed.tool_name, // tool_name field holds the original prompt name
                params.arguments,
            )
            .await
        {
            Ok(result) => {
                // Success - track the single MCP that was called
                McpTrackedResponse::with_single_mcp(Self::success_response(id, &result), mcp_id)
            }
            Err(e) => {
                // Error during prompt get - still track the MCP (it was accessed)
                McpTrackedResponse::with_single_mcp(
                    JsonRpcResponse::error(
                        id,
                        JsonRpcError::internal_error(format!(
                            "Failed to get prompt '{}' from MCP '{}': {}",
                            parsed.tool_name, parsed.mcp_name, e
                        )),
                    ),
                    mcp_id,
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
    use super::*;

    #[test]
    #[ignore = "requires database pool mocking - TODO: implement with sqlx test fixtures"]
    fn test_parse_auth_bearer() {
        // This test requires a mocked database pool which isn't easily available
        // in unit tests. For now, this functionality is covered by integration tests.
        // TODO: Implement proper unit test with sqlx test fixtures or mock pool
    }
}
