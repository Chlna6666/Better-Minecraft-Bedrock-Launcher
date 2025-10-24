use std::sync::Arc;
use reqwest::{Client, header};
use tracing::debug;

use crate::result::{CoreError, CoreResult};
use std::time::Duration;
use tokio::time::sleep;
use crate::downloads::cancel::{is_cancelled}; // 使用新的 is_cancelled 接口
use crate::downloads::WuClient::protocol::WuProtocol;

pub struct WuClient {
    client: Client,
    protocol: WuProtocol,
}

impl WuClient {
    pub fn with_client(client: Client) -> Self {
        Self {
            client,
            protocol: WuProtocol::new(),
        }
    }

    /// 等待取消的异步函数：传入可选的 per-task cancel flag（Some）或 None（表示检查全局）
    async fn wait_cancelled(cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>) {
        // 轮询检查 atomic 标志；间隔可以根据需要调整（例如 50ms 或 100ms）
        while !is_cancelled(cancel_flag.as_ref()) {
            sleep(Duration::from_millis(50)).await;
        }
    }

    /// 获取下载 URL：新增 cancel_flag 参数，传 Some(flag) 使用 per-task，传 None 使用全局取消开关
    pub async fn get_download_url(
        &self,
        update_id: &str,
        revision: &str,
        cancel_flag: Option<Arc<std::sync::atomic::AtomicBool>>,
    ) -> Result<CoreResult<String>, CoreError> {
        let request_xml = self.protocol.build_download_request(update_id, revision);

        for attempt in 1..=3 {
            debug!("第 {} 次请求下载 URL", attempt);

            // 先快速检查一次（避免不必要的工作）
            if is_cancelled(cancel_flag.as_ref()) {
                debug!("下载已取消（请求前）");
                return Ok(CoreResult::Cancelled);
            }

            // 发起请求的 future（未 await）
            let send_fut = self
                .client
                .post("https://fe3.delivery.mp.microsoft.com/ClientWebService/client.asmx/secured")
                .header(header::CONTENT_TYPE, "application/soap+xml")
                .body(request_xml.clone())
                .send();

            // 在请求发送阶段可取消
            let send_result = tokio::select! {
                // 传入 clone（cheap）— 每次都创建新的等待 future，避免 move/重用问题
                _ = Self::wait_cancelled(cancel_flag.clone()) => {
                    debug!("下载已取消（发送阶段）");
                    return Ok(CoreResult::Cancelled);
                }

                // 请求完成
                res = send_fut => res
            };

            match send_result {
                Ok(resp) => {
                    // 检查 HTTP 状态
                    match resp.error_for_status() {
                        Ok(valid_resp) => {
                            // 在读取 body 阶段也支持取消
                            let text_result = tokio::select! {
                                _ = Self::wait_cancelled(cancel_flag.clone()) => {
                                    debug!("下载已取消（读取 body 阶段）");
                                    return Ok(CoreResult::Cancelled);
                                }
                                txt = valid_resp.text() => txt
                            };

                            // 读取 body 出错（会被 ? 转 CoreError::Request，前提是有 From impl）
                            let xml = text_result?;
                            debug!("响应 XML: {}", xml);

                            // 解析 XML（协议层函数的错误请确认能转为 CoreError::Xml）
                            let urls = self.protocol.parse_download_response(&xml)?;
                            debug!("解析到的 URL 列表: {:?}", urls);

                            if let Some(url) = urls
                                .into_iter()
                                .find(|u| u.starts_with("http://tlu.dl.delivery.mp.microsoft.com/"))
                            {
                                return Ok(CoreResult::Success(url));
                            } else if attempt == 3 {
                                return Err(CoreError::BadUpdateIdentity);
                            }
                        }
                        Err(e) => {
                            debug!("第 {} 次请求返回错误状态: {}", attempt, e);
                            if attempt == 3 {
                                return Err(CoreError::Request(e));
                            }
                        }
                    }
                }
                Err(err) => {
                    debug!("第 {} 次请求失败: {}", attempt, err);
                    if attempt == 3 {
                        return Err(CoreError::Request(err));
                    }
                }
            }

            let backoff = 500 * attempt * attempt;
            sleep(Duration::from_millis(backoff as u64)).await;
        }

        Err(CoreError::BadUpdateIdentity)
    }
}
