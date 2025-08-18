use std::sync::atomic::Ordering;
use reqwest::{Client, header};
use tracing::debug;

use crate::core::result::{CoreError, CoreResult};
use std::time::Duration;
use tokio::time::sleep;
use crate::core::downloads::cancel::CANCEL_DOWNLOAD;
use crate::core::downloads::WuClient::protocol::WuProtocol;

pub struct WuClient {
    client: Client,
    protocol: WuProtocol,
}

impl WuClient {
    /// 通过传入的 Client 构造 WuClient（支持代理）
    pub fn with_client(client: Client) -> Self {
        Self {
            client,
            protocol: WuProtocol::new(),
        }
    }

    /// 获取下载 URL，带重试逻辑
    /// 返回类型改为 Result<CoreResult<String>, CoreError>
    pub async fn get_download_url(
        &self,
        update_id: &str,
        revision: &str,
    ) -> Result<CoreResult<String>, CoreError> {
        let request_xml = self.protocol.build_download_request(update_id, revision);

        for attempt in 1..=3 {
            debug!("第 {} 次请求下载 URL", attempt);

            // 如果外部请求已取消，则立即返回 Cancelled（作为正常分支）
            if CANCEL_DOWNLOAD.load(Ordering::Relaxed) {
                debug!("下载已取消，停止获取 URL");
                return Ok(CoreResult::Cancelled);
            }

            let result = self
                .client
                .post("https://fe3.delivery.mp.microsoft.com/ClientWebService/client.asmx/secured")
                .header(header::CONTENT_TYPE, "application/soap+xml")
                .body(request_xml.clone())
                .send()
                .await
                .and_then(|resp| resp.error_for_status());

            match result {
                Ok(response) => {
                    // response.text().await 会在出错时通过 ? 转为 CoreError::Request
                    let xml = response.text().await?;
                    debug!("响应 XML: {}", xml);

                    // parse_download_response 返回 Result<Vec<String>, xml::Error> (示例)
                    // 使用 ? 将解析错误上抛为 CoreError::Xml（按你的 CoreError 定义）
                    let urls = self.protocol.parse_download_response(&xml)?;
                    debug!("解析到的 URL 列表: {:?}", urls);

                    if let Some(url) = urls
                        .into_iter()
                        .find(|u| u.starts_with("http://tlu.dl.delivery.mp.microsoft.com/"))
                    {
                        // 找到合适的 URL —— 作为 Success 返回，并携带 String
                        return Ok(CoreResult::Success(url));
                    } else if attempt == 3 {
                        // 三次尝试后仍然没有合适 URL —— 视为 BadUpdateIdentity（保持原有行为）
                        return Err(CoreError::BadUpdateIdentity);
                    }
                }
                Err(err) => {
                    debug!("第 {} 次请求或状态检查失败: {}", attempt, err);
                    if attempt == 3 {
                        // 三次都失败，向上返回请求错误（保持原有行为）
                        return Err(CoreError::Request(err));
                    }
                }
            }

            let backoff = 500 * attempt * attempt;
            sleep(Duration::from_millis(backoff as u64)).await;
        }

        // 理论上不会到这里，但保底返回 BadUpdateIdentity
        Err(CoreError::BadUpdateIdentity)
    }
}
