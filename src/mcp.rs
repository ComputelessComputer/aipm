use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcRequest {
    jsonrpc: String,
    id: u64,
    method: String,
    params: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Email {
    pub id: String,
    pub subject: String,
    pub sender: String,
    pub date: String,
    pub content: Option<String>,
    pub is_read: bool,
}

pub struct McpClient {
    child: Arc<Mutex<Child>>,
    stdin: Arc<Mutex<ChildStdin>>,
    stdout: Arc<Mutex<BufReader<ChildStdout>>>,
    next_id: Arc<Mutex<u64>>,
}

impl McpClient {
    pub fn spawn(python_path: &str, script_path: &str) -> Result<McpClient, String> {
        let mut child = Command::new(python_path)
            .arg(script_path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("Failed to spawn MCP server: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "Failed to capture stdin".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "Failed to capture stdout".to_string())?;

        let stdout = BufReader::new(stdout);

        let client = McpClient {
            child: Arc::new(Mutex::new(child)),
            stdin: Arc::new(Mutex::new(stdin)),
            stdout: Arc::new(Mutex::new(stdout)),
            next_id: Arc::new(Mutex::new(1)),
        };

        client.initialize()?;

        Ok(client)
    }

    fn initialize(&self) -> Result<(), String> {
        let params = serde_json::json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "aipm",
                "version": "0.7.2"
            }
        });

        self.call("initialize", Some(params))?;
        self.notify("notifications/initialized", None)?;
        Ok(())
    }

    fn call(
        &self,
        method: &str,
        params: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, String> {
        let id = {
            let mut next_id = self.next_id.lock().unwrap();
            let current = *next_id;
            *next_id += 1;
            current
        };

        let request = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            id,
            method: method.to_string(),
            params,
        };

        let mut stdin = self.stdin.lock().unwrap();
        let request_json = serde_json::to_string(&request)
            .map_err(|e| format!("Failed to serialize request: {e}"))?;

        stdin
            .write_all(request_json.as_bytes())
            .map_err(|e| format!("Failed to write request: {e}"))?;
        stdin
            .write_all(b"\n")
            .map_err(|e| format!("Failed to write newline: {e}"))?;
        stdin.flush().map_err(|e| format!("Failed to flush: {e}"))?;

        drop(stdin);

        let mut stdout = self.stdout.lock().unwrap();
        let mut line = String::new();
        stdout
            .read_line(&mut line)
            .map_err(|e| format!("Failed to read response: {e}"))?;

        let response: JsonRpcResponse =
            serde_json::from_str(&line).map_err(|e| format!("Failed to parse response: {e}"))?;

        if let Some(error) = response.error {
            return Err(format!("MCP error: {}", error.message));
        }

        response
            .result
            .ok_or_else(|| "No result in response".to_string())
    }

    fn notify(&self, method: &str, params: Option<serde_json::Value>) -> Result<(), String> {
        let notification = serde_json::json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        });

        let mut stdin = self.stdin.lock().unwrap();
        let notification_json = serde_json::to_string(&notification)
            .map_err(|e| format!("Failed to serialize notification: {e}"))?;

        stdin
            .write_all(notification_json.as_bytes())
            .map_err(|e| format!("Failed to write notification: {e}"))?;
        stdin
            .write_all(b"\n")
            .map_err(|e| format!("Failed to write newline: {e}"))?;
        stdin.flush().map_err(|e| format!("Failed to flush: {e}"))?;

        Ok(())
    }

    pub fn get_recent_emails(&self, limit: u32) -> Result<Vec<Email>, String> {
        let params = serde_json::json!({
            "arguments": {
                "limit": limit
            }
        });

        let result = self.call(
            "tools/call",
            Some(serde_json::json!({
                "name": "get_recent_emails",
                "arguments": params["arguments"]
            })),
        )?;

        let content = result["content"]
            .as_array()
            .ok_or_else(|| "No content in response".to_string())?;

        let text = content
            .iter()
            .find(|item| item["type"] == "text")
            .and_then(|item| item["text"].as_str())
            .ok_or_else(|| "No text in content".to_string())?;

        let emails: Vec<Email> =
            serde_json::from_str(text).map_err(|e| format!("Failed to parse emails: {e}"))?;

        Ok(emails)
    }

    pub fn get_email_content(&self, email_id: &str) -> Result<Email, String> {
        let params = serde_json::json!({
            "arguments": {
                "email_id": email_id
            }
        });

        let result = self.call(
            "tools/call",
            Some(serde_json::json!({
                "name": "get_email_with_content",
                "arguments": params["arguments"]
            })),
        )?;

        let content = result["content"]
            .as_array()
            .ok_or_else(|| "No content in response".to_string())?;

        let text = content
            .iter()
            .find(|item| item["type"] == "text")
            .and_then(|item| item["text"].as_str())
            .ok_or_else(|| "No text in content".to_string())?;

        let email: Email =
            serde_json::from_str(text).map_err(|e| format!("Failed to parse email: {e}"))?;

        Ok(email)
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
