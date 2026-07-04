use crate::http::proxy::get_blocking_client_for_proxy;
use futures::FutureExt as _;
use futures::future::BoxFuture;
use futures::io::AsyncReadExt as _;
use gpui::http_client;
use gpui::http_client::HttpClient;
use std::sync::Arc;

pub fn create_gpui_http_client() -> Arc<dyn HttpClient> {
    Arc::new(ReqwestGpuiClient::new())
}

struct ReqwestGpuiClient {
    user_agent: http_client::http::HeaderValue,
}

impl ReqwestGpuiClient {
    fn new() -> Self {
        Self {
            user_agent: http_client::http::HeaderValue::from_static("BMCBL"),
        }
    }
}

impl HttpClient for ReqwestGpuiClient {
    fn type_name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }

    fn user_agent(&self) -> Option<&http_client::http::HeaderValue> {
        Some(&self.user_agent)
    }

    fn send(
        &self,
        request: http_client::Request<http_client::AsyncBody>,
    ) -> BoxFuture<'static, anyhow::Result<http_client::Response<http_client::AsyncBody>>> {
        async move {
            let (parts, mut body) = request.into_parts();
            let url = parts.uri.to_string();
            let method = reqwest::Method::from_bytes(parts.method.as_str().as_bytes())?;

            let mut body_bytes = Vec::new();
            body.read_to_end(&mut body_bytes).await?;

            send_blocking_request(method, url, parts.headers, body_bytes).await
        }
        .boxed()
    }

    fn proxy(&self) -> Option<&http_client::Url> {
        None
    }
}

async fn send_blocking_request(
    method: reqwest::Method,
    url: String,
    headers: http_client::http::HeaderMap,
    body_bytes: Vec<u8>,
) -> anyhow::Result<http_client::Response<http_client::AsyncBody>> {
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => handle
            .spawn_blocking(move || build_blocking_response(method, url, headers, body_bytes))
            .await
            .map_err(|error| anyhow::anyhow!("http task failed: {error}"))?,
        Err(_) => {
            let (tx, rx) = futures::channel::oneshot::channel();
            std::thread::Builder::new()
                .name("bmcbl-gpui-http".to_string())
                .spawn(move || {
                    let result = build_blocking_response(method, url, headers, body_bytes);
                    if tx.send(result).is_err() {
                        tracing::debug!("GPUI HTTP receiver dropped before request completed");
                    }
                })
                .map_err(|error| anyhow::anyhow!("failed to spawn HTTP task: {error}"))?;

            rx.await
                .map_err(|_| anyhow::anyhow!("http task cancelled"))?
        }
    }
}

fn build_blocking_response(
    method: reqwest::Method,
    url: String,
    headers: http_client::http::HeaderMap,
    body_bytes: Vec<u8>,
) -> anyhow::Result<http_client::Response<http_client::AsyncBody>> {
    let client = get_blocking_client_for_proxy()
        .map_err(|error| anyhow::anyhow!("blocking client init failed: {error}"))?;
    let mut builder = client.request(method, url);
    for (name, value) in &headers {
        builder = builder.header(name, value);
    }
    if !body_bytes.is_empty() {
        builder = builder.body(body_bytes);
    }

    let response = builder.send()?;
    let status =
        http_client::StatusCode::from_u16(response.status().as_u16()).map_err(|error| {
            anyhow::anyhow!(
                "invalid status code {}: {}",
                response.status().as_u16(),
                error
            )
        })?;

    let mut output = http_client::Response::builder().status(status);
    for (name, value) in response.headers() {
        output = output.header(name, value);
    }

    let bytes = response.bytes()?;
    Ok(output.body(http_client::AsyncBody::from(bytes.to_vec()))?)
}
