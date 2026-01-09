use anyhow::{Context, Result};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use http_body_util::{BodyExt, BodyStream, Full};
use hyper::{body::Bytes, client::conn::http1, Method, Request, StatusCode, Uri};
use hyper_util::rt::TokioIo;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::PathBuf;
use tokio::net::UnixStream;

use katana_models::{
    CreateInstanceRequest, ErrorResponse, InstanceResponse, ListInstancesResponse, LogsResponse,
    StatsResponse,
};

#[derive(Debug)]
pub struct Client {
    socket_path: PathBuf,
}

impl Client {
    pub fn new(socket_path: impl Into<PathBuf>) -> Self {
        Self {
            socket_path: socket_path.into(),
        }
    }

    /// Create a new instance
    pub async fn create_instance(
        &self,
        request: CreateInstanceRequest,
    ) -> Result<InstanceResponse> {
        let body = serde_json::to_value(&request)?;
        self.post("/api/v1/instances", Some(body)).await
    }

    /// List all instances
    pub async fn list_instances(&self) -> Result<ListInstancesResponse> {
        self.get("/api/v1/instances").await
    }

    /// Get a specific instance by name
    pub async fn get_instance(&self, name: &str) -> Result<InstanceResponse> {
        let path = format!("/api/v1/instances/{}", name);
        self.get(&path).await
    }

    /// Delete an instance
    pub async fn delete_instance(&self, name: &str) -> Result<()> {
        let path = format!("/api/v1/instances/{}", name);
        self.delete(&path).await
    }

    /// Start an instance
    pub async fn start_instance(&self, name: &str) -> Result<InstanceResponse> {
        let path = format!("/api/v1/instances/{}/start", name);
        self.post(&path, None).await
    }

    /// Stop an instance
    pub async fn stop_instance(&self, name: &str) -> Result<InstanceResponse> {
        let path = format!("/api/v1/instances/{}/stop", name);
        self.post(&path, None).await
    }

    /// Get logs for an instance
    pub async fn get_logs(&self, name: &str, tail: Option<usize>) -> Result<LogsResponse> {
        let tail_param = tail.unwrap_or(100);
        let path = format!("/api/v1/instances/{}/logs?tail={}", name, tail_param);
        self.get(&path).await
    }

    /// Stream logs for an instance using Server-Sent Events
    pub async fn stream_logs<F>(&self, name: &str, tail: Option<usize>, callback: F) -> Result<()>
    where
        F: FnMut(String, String),
    {
        let tail_param = tail.unwrap_or(20);
        let path = format!("/api/v1/instances/{}/logs/stream?tail={}", name, tail_param);
        self.stream_sse(&path, callback).await
    }

    /// Get statistics for an instance
    pub async fn get_stats(&self, name: &str) -> Result<StatsResponse> {
        let path = format!("/api/v1/instances/{}/stats", name);
        self.get(&path).await
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        self.request(Method::GET, path, None).await
    }

    async fn post<T: DeserializeOwned>(&self, path: &str, body: Option<Value>) -> Result<T> {
        self.request(Method::POST, path, body).await
    }

    async fn delete(&self, path: &str) -> Result<()> {
        let (status, _) = self.request_raw(Method::DELETE, path, None).await?;

        if !status.is_success() {
            anyhow::bail!("HTTP {}: Delete failed", status);
        }

        Ok(())
    }

    async fn request<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<T> {
        let (status, response_body) = self.request_raw(method, path, body).await?;

        if !status.is_success() {
            // Try to parse error response
            if let Ok(error) = serde_json::from_value::<ErrorResponse>(response_body.clone()) {
                anyhow::bail!("{}", error.error.message);
            } else {
                anyhow::bail!("HTTP {}: {}", status, response_body);
            }
        }

        // Deserialize to the target type
        serde_json::from_value(response_body).context("Failed to deserialize response")
    }

    async fn request_raw(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<(StatusCode, Value)> {
        // Connect to UNIX socket
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .context(format!(
                "Cannot connect to daemon at unix: {}. Is the daemon running?",
                self.socket_path.display()
            ))?;

        // Build request
        let uri = Uri::builder()
            .scheme("http")
            .authority("localhost")
            .path_and_query(path)
            .build()?;

        let mut req_builder = Request::builder().method(method).uri(uri);

        let request = if let Some(json) = body {
            req_builder = req_builder.header("content-type", "application/json");
            let json_bytes = serde_json::to_vec(&json)?;
            req_builder.body(Full::new(Bytes::from(json_bytes)))?
        } else {
            req_builder.body(Full::new(Bytes::new()))?
        };

        // Send request using oneshot connection
        let (mut sender, conn) = http1::handshake(TokioIo::new(stream)).await?;

        // Spawn connection task
        tokio::spawn(async move {
            if let Err(err) = conn.await {
                eprintln!("Connection error: {:?}", err);
            }
        });

        let response = sender.send_request(request).await?;
        let status = response.status();

        // Read response body
        let body_bytes = response.into_body().collect().await?.to_bytes();

        let response_body: Value = if body_bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body_bytes).context("Failed to parse response JSON")?
        };

        Ok((status, response_body))
    }

    async fn stream_sse<F>(&self, path: &str, mut callback: F) -> Result<()>
    where
        F: FnMut(String, String),
    {
        // Connect to UNIX socket
        let stream = UnixStream::connect(&self.socket_path)
            .await
            .context("Cannot connect to daemon at unix: {socket_path}. Is the daemon running?")?;

        let (mut sender, conn) = http1::handshake(TokioIo::new(stream)).await?;

        tokio::spawn(async move {
            if let Err(err) = conn.await {
                eprintln!("Connection error: {:?}", err);
            }
        });

        // Build SSE request
        let uri = Uri::builder()
            .scheme("http")
            .authority("localhost")
            .path_and_query(path)
            .build()?;

        let request = Request::builder()
            .method(Method::GET)
            .uri(uri)
            .header("accept", "text/event-stream")
            .body(Full::new(Bytes::new()))?;

        // Send request
        let response = sender.send_request(request).await?;

        // Parse SSE stream - convert Incoming to byte stream
        let body = response.into_body();
        let byte_stream = BodyStream::new(body).map(|result| {
            result
                .map(|frame| frame.into_data().unwrap_or_default())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))
        });
        let mut event_stream = byte_stream.eventsource();

        while let Some(event) = event_stream.next().await {
            match event {
                Ok(event) => {
                    let event_type = if event.event.is_empty() {
                        "message".to_string()
                    } else {
                        event.event
                    };
                    let data = event.data;
                    callback(event_type, data);
                }
                Err(e) => {
                    eprintln!("Stream error: {}", e);
                    break;
                }
            }
        }

        Ok(())
    }
}
