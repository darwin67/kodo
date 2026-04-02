use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context as _, Result};
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tracing::{debug, info, warn};

use kodo_llm::provider::Provider;
use kodo_llm::types::Message;

use crate::agent::{Agent, AgentEvent};
use crate::mode::Mode;

/// Request to a subagent
#[derive(Debug, Clone)]
pub struct SubagentRequest {
    /// Unique ID for this request
    pub id: String,
    /// Task description for the subagent
    pub task: String,
    /// System prompt override (optional)
    pub system_prompt: Option<String>,
    /// Initial context/messages (optional)
    pub context: Vec<Message>,
    /// Maximum time allowed for completion
    pub timeout: Duration,
    /// Mode for the subagent (Plan/Build)
    pub mode: Mode,
}

/// Response from a subagent
#[derive(Debug, Clone)]
pub struct SubagentResponse {
    /// Request ID this response corresponds to
    pub request_id: String,
    /// Whether the task completed successfully
    pub success: bool,
    /// Final response/summary from the subagent
    pub result: String,
    /// All messages exchanged during the task
    pub messages: Vec<Message>,
    /// Any error that occurred
    pub error: Option<String>,
}

/// Subagent manager for spawning and managing isolated agent tasks
pub struct SubagentManager {
    provider: Arc<dyn Provider>,
    model: String,
    max_concurrent: usize,
    active_count: Arc<tokio::sync::Mutex<usize>>,
}

impl SubagentManager {
    pub fn new(provider: Arc<dyn Provider>, model: String) -> Self {
        Self {
            provider,
            model,
            max_concurrent: 5, // Default to 5 concurrent subagents
            active_count: Arc::new(tokio::sync::Mutex::new(0)),
        }
    }

    pub fn with_max_concurrent(mut self, max: usize) -> Self {
        self.max_concurrent = max;
        self
    }

    /// Spawn a subagent to handle a specific task
    pub async fn spawn(&self, request: SubagentRequest) -> Result<SubagentResponse> {
        // Check concurrent limit
        {
            let mut count = self.active_count.lock().await;
            if *count >= self.max_concurrent {
                return Err(anyhow::anyhow!(
                    "Maximum concurrent subagents ({}) reached",
                    self.max_concurrent
                ));
            }
            *count += 1;
        }

        // Decrement count when done
        let active_count = self.active_count.clone();
        let _guard = scopeguard::guard((), move |_| {
            tokio::spawn(async move {
                let mut count = active_count.lock().await;
                *count = count.saturating_sub(1);
            });
        });

        info!(
            "Spawning subagent for task: {} (timeout: {:?})",
            request.task, request.timeout
        );

        // Create isolated agent
        let mut agent = Agent::new(self.provider.clone())
            .with_model(&self.model)
            .with_mode(request.mode);

        // Set custom system prompt if provided
        if let Some(prompt) = &request.system_prompt {
            agent.set_system_prompt(prompt);
        }

        // Add initial context
        for msg in &request.context {
            agent.add_message(msg.clone());
        }

        // Create channels for communication
        let (tx, mut rx) = mpsc::unbounded_channel::<AgentEvent>();
        let (result_tx, result_rx) = oneshot::channel::<SubagentResponse>();

        // Spawn the subagent task
        let request_id = request.id.clone();
        let task = request.task.clone();

        tokio::spawn(async move {
            let start = std::time::Instant::now();
            let mut all_text = String::new();
            let mut error = None;
            let mut success = true;

            // Process the task
            match agent.process_message(&task, Some(&tx)).await {
                Ok(_) => {
                    debug!("Subagent completed task successfully");
                }
                Err(e) => {
                    warn!("Subagent task failed: {}", e);
                    error = Some(format!("{:?}", e));
                    success = false;
                }
            }

            // Collect all streamed text
            while let Ok(event) = rx.try_recv() {
                if let AgentEvent::TextDelta(text) = event {
                    all_text.push_str(&text);
                }
            }

            let response = SubagentResponse {
                request_id,
                success,
                result: if all_text.is_empty() {
                    error
                        .clone()
                        .unwrap_or_else(|| "No response generated".to_string())
                } else {
                    all_text
                },
                messages: agent.messages().to_vec(),
                error,
            };

            info!(
                "Subagent completed in {:?} - success: {}",
                start.elapsed(),
                success
            );

            let _ = result_tx.send(response);
        });

        // Wait for result with timeout
        match timeout(request.timeout, result_rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(anyhow::anyhow!("Subagent task was cancelled")),
            Err(_) => {
                warn!("Subagent task timed out after {:?}", request.timeout);
                Ok(SubagentResponse {
                    request_id: request.id,
                    success: false,
                    result: format!("Task timed out after {:?}", request.timeout),
                    messages: vec![],
                    error: Some("Timeout".to_string()),
                })
            }
        }
    }

    /// Spawn multiple subagents in parallel
    pub async fn spawn_many(
        &self,
        requests: Vec<SubagentRequest>,
    ) -> Vec<Result<SubagentResponse>> {
        let handles: Vec<_> = requests
            .into_iter()
            .map(|req| {
                let manager = self.clone();
                tokio::spawn(async move { manager.spawn(req).await })
            })
            .collect();

        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => results.push(Err(anyhow::anyhow!("Task failed: {}", e))),
            }
        }

        results
    }
}

impl Clone for SubagentManager {
    fn clone(&self) -> Self {
        Self {
            provider: self.provider.clone(),
            model: self.model.clone(),
            max_concurrent: self.max_concurrent,
            active_count: self.active_count.clone(),
        }
    }
}

/// Tool for spawning subagents from within the main agent
pub struct SubagentTool {
    manager: SubagentManager,
}

impl SubagentTool {
    pub fn new(provider: Arc<dyn Provider>, model: String) -> Self {
        Self {
            manager: SubagentManager::new(provider, model),
        }
    }
}

impl kodo_tools::tool::Tool for SubagentTool {
    fn name(&self) -> &str {
        "spawn_subagent"
    }

    fn description(&self) -> &str {
        "Spawn an isolated subagent to handle a specific task independently. \
         Useful for parallel processing or delegating specialized work."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "task": {
                    "type": "string",
                    "description": "The task for the subagent to complete"
                },
                "mode": {
                    "type": "string",
                    "enum": ["plan", "build"],
                    "description": "Execution mode for the subagent (default: plan)"
                },
                "timeout_seconds": {
                    "type": "integer",
                    "description": "Maximum time allowed in seconds (default: 300)"
                },
                "system_prompt": {
                    "type": "string",
                    "description": "Custom system prompt for the subagent (optional)"
                }
            },
            "required": ["task"]
        })
    }

    fn execute(
        &self,
        params: serde_json::Value,
        _ctx: &kodo_tools::tool::ToolContext,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<kodo_tools::tool::ToolOutput>> + Send + '_>,
    > {
        Box::pin(async move {
            let task = params["task"]
                .as_str()
                .context("Missing 'task' parameter")?;

            let mode = params["mode"]
                .as_str()
                .and_then(|m| match m {
                    "plan" => Some(Mode::Plan),
                    "build" => Some(Mode::Build),
                    _ => None,
                })
                .unwrap_or(Mode::Plan);

            let timeout_secs = params["timeout_seconds"].as_u64().unwrap_or(300);

            let system_prompt = params["system_prompt"].as_str().map(|s| s.to_string());

            let request = SubagentRequest {
                id: uuid::Uuid::new_v4().to_string(),
                task: task.to_string(),
                system_prompt,
                context: vec![],
                timeout: Duration::from_secs(timeout_secs),
                mode,
            };

            match self.manager.spawn(request).await {
                Ok(response) => {
                    if response.success {
                        Ok(kodo_tools::tool::ToolOutput {
                            content: format!(
                                "Subagent completed successfully:\n\n{}",
                                response.result
                            ),
                            success: true,
                        })
                    } else {
                        Ok(kodo_tools::tool::ToolOutput {
                            content: format!(
                                "Subagent task failed: {}\n\nResult: {}",
                                response.error.as_deref().unwrap_or("Unknown error"),
                                response.result
                            ),
                            success: false,
                        })
                    }
                }
                Err(e) => Ok(kodo_tools::tool::ToolOutput {
                    content: format!("Failed to spawn subagent: {}", e),
                    success: false,
                }),
            }
        })
    }

    fn permission_level(&self) -> kodo_tools::tool::PermissionLevel {
        kodo_tools::tool::PermissionLevel::Execute
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_subagent_request_creation() {
        let request = SubagentRequest {
            id: "test-123".to_string(),
            task: "Analyze this code".to_string(),
            system_prompt: None,
            context: vec![],
            timeout: Duration::from_secs(60),
            mode: Mode::Plan,
        };

        assert_eq!(request.id, "test-123");
        assert_eq!(request.task, "Analyze this code");
        assert_eq!(request.timeout, Duration::from_secs(60));
        assert_eq!(request.mode, Mode::Plan);
    }

    #[tokio::test]
    async fn test_subagent_response_creation() {
        let response = SubagentResponse {
            request_id: "test-123".to_string(),
            success: true,
            result: "Analysis complete".to_string(),
            messages: vec![],
            error: None,
        };

        assert_eq!(response.request_id, "test-123");
        assert!(response.success);
        assert_eq!(response.result, "Analysis complete");
        assert!(response.error.is_none());
    }
}
