use serde::{Deserialize, Serialize};

/// Backend for communicating with Claude.
enum Backend {
    /// Direct HTTP API (requires ANTHROPIC_API_KEY)
    Api {
        api_key: String,
        model: String,
        http: reqwest::Client,
    },
    /// Shell out to `claude -p` (uses Claude Code's existing auth)
    Cli { model: String },
}

/// Client for sending prompts to Claude, with automatic fallback.
///
/// Tries the HTTP API first (if an API key is available), otherwise
/// falls back to `claude -p` which uses Claude Code's built-in auth.
pub struct ClaudeClient {
    backend: Backend,
}

#[derive(Serialize)]
struct MessagesRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<Message>,
}

#[derive(Serialize)]
struct Message {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct MessagesResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

impl ClaudeClient {
    /// Create a client using the HTTP API directly.
    pub fn with_api_key(api_key: String, model: String) -> Self {
        Self {
            backend: Backend::Api {
                api_key,
                model,
                http: reqwest::Client::new(),
            },
        }
    }

    /// Create a client that shells out to `claude -p`.
    pub fn with_cli(model: String) -> Self {
        Self {
            backend: Backend::Cli { model },
        }
    }

    /// Create the best available client: API key if set, otherwise CLI fallback.
    pub fn auto(api_key_env: &str, model: String) -> Self {
        match std::env::var(api_key_env) {
            Ok(key) if !key.is_empty() => {
                tracing::info!("Using Claude HTTP API (key from {api_key_env})");
                Self::with_api_key(key, model)
            }
            _ => {
                tracing::info!("No API key found, using `claude -p` CLI fallback");
                Self::with_cli(model)
            }
        }
    }

    /// Send a prompt to Claude and get the text response.
    pub async fn complete(&self, prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
        match &self.backend {
            Backend::Api {
                api_key,
                model,
                http,
            } => Self::complete_api(http, api_key, model, prompt).await,
            Backend::Cli { model } => Self::complete_cli(model, prompt).await,
        }
    }

    async fn complete_api(
        http: &reqwest::Client,
        api_key: &str,
        model: &str,
        prompt: &str,
    ) -> Result<String, Box<dyn std::error::Error>> {
        let request = MessagesRequest {
            model: model.to_string(),
            max_tokens: 4096,
            messages: vec![Message {
                role: "user".to_string(),
                content: prompt.to_string(),
            }],
        };

        let response = http
            .post("https://api.anthropic.com/v1/messages")
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01")
            .header("content-type", "application/json")
            .json(&request)
            .send()
            .await?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(format!("Claude API error {status}: {body}").into());
        }

        let resp: MessagesResponse = response.json().await?;

        let text = resp
            .content
            .iter()
            .filter_map(|b| {
                if b.block_type == "text" {
                    b.text.clone()
                } else {
                    None
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        Ok(text)
    }

    async fn complete_cli(model: &str, prompt: &str) -> Result<String, Box<dyn std::error::Error>> {
        use tokio::io::AsyncWriteExt;

        let claude_bin = dirs::home_dir()
            .map(|h| h.join(".local/bin/claude"))
            .filter(|p| p.exists())
            .unwrap_or_else(|| "claude".into());

        let mut child = tokio::process::Command::new(claude_bin)
            .args([
                "-p",
                "--model",
                model,
                "--no-session-persistence",
                "--output-format",
                "text",
                "--system-prompt",
                "You are a knowledge extraction engine. You MUST respond with ONLY valid JSON, no markdown, no explanation, no prose. If the input contains no extractable knowledge, respond with: {\"roots\": []}",
            ])
            .env_remove("CLAUDECODE")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()?;

        // Pipe the prompt via stdin (handles large prompts better than CLI args)
        if let Some(mut stdin) = child.stdin.take() {
            stdin.write_all(prompt.as_bytes()).await?;
            // Drop stdin to signal EOF
        }

        let output = child.wait_with_output().await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(format!(
                "claude CLI exited with {}: {}",
                output.status,
                stderr.chars().take(500).collect::<String>()
            )
            .into());
        }

        let text = String::from_utf8(output.stdout)?;
        Ok(text)
    }
}
