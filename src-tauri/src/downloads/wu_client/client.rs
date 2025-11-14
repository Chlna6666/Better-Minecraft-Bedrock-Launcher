use reqwest::{Client, header};
use tracing::debug;

use crate::result::{CoreError, CoreResult};
use std::time::Duration;
use tokio::time::sleep;
use crate::downloads::wu_client::protocol::WuProtocol;
use crate::tasks::task_manager::{finish_task, is_cancelled, update_progress};

/// 现在通过 task_manager 管理取消与阶段信息
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

    /// 等待取消的异步函数：通过 task_id 查询 task_manager
    async fn wait_cancelled(task_id: String) {
        // 只要任务未标记为 cancelled 就循环等待
        loop {
            if is_cancelled(&task_id) {
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    /// 获取下载 URL（使用 task_manager 的 task_id 进行取消检查与阶段上报）
    /// 上层调用示例：
    /// let task_id = create_task("resolving_url", None);
    /// wu_client.get_download_url(update_id, revision, &task_id).await
    pub async fn get_download_url(
        &self,
        update_id: &str,
        revision: &str,
        task_id: &str
    ) -> Result<CoreResult<String>, CoreError> {
        let request_xml = self.protocol.build_download_request(update_id, revision);

        // 把阶段标为 resolving_url
        update_progress(task_id, 0, None, Some("resolving_url"));

        for attempt in 1..=3 {
            debug!("task={} 第 {} 次请求下载 URL", task_id, attempt);

            // 先快速检查一次取消
            if is_cancelled(task_id) {
                debug!("task={} 已取消（请求前）", task_id);
                finish_task(task_id, "cancelled", Some("user cancelled".into()));
                return Ok(CoreResult::Cancelled);
            }

            // 发起请求（未 await）
            let send_fut = self
                .client
                .post("https://fe3.delivery.mp.microsoft.com/ClientWebService/client.asmx/secured")
                .header(header::CONTENT_TYPE, "application/soap+xml")
                .body(request_xml.clone())
                .send();

            // 在请求发送阶段也支持取消（通过 task_manager）
            let send_result = tokio::select! {
            _ = Self::wait_cancelled(task_id.to_string()) => {
                debug!("task={} 下载已取消（发送阶段）", task_id);
                finish_task(task_id, "cancelled", Some("user cancelled".into()));
                return Ok(CoreResult::Cancelled);
            }
            r = send_fut => r
        };

            match send_result {
                Ok(resp) => {
                    // 检查 HTTP 状态
                    match resp.error_for_status() {
                        Ok(valid_resp) => {
                            // 上报阶段：reading_body
                            update_progress(task_id, 0, None, Some("reading_body"));

                            // 在读取 body 阶段也支持取消
                            let text_result = tokio::select! {
                            _ = Self::wait_cancelled(task_id.to_string()) => {
                                debug!("task={} 已取消（读取 body 阶段）", task_id);
                                finish_task(task_id, "cancelled", Some("user cancelled".into()));
                                return Ok(CoreResult::Cancelled);
                            }
                            txt = valid_resp.text() => txt
                        };

                            let xml = text_result?;
                            debug!("task={} 响应 XML: {}", task_id, xml);

                            // 上报阶段：parsing
                            update_progress(task_id, 0, None, Some("parsing"));

                            // 解析 XML（协议层函数的错误应能转为 CoreError）
                            let urls = match self.protocol.parse_download_response(&xml) {
                                Ok(u) => u,
                                Err(e) => {
                                    debug!("task={} 解析 XML 失败: {:?}", task_id, e);
                                    // 解析失败通常是致命的，标 error 并返回 Err
                                    finish_task(task_id, "error", Some("parse response failed".into()));
                                    return Err(e);
                                }
                            };
                            debug!("task={} 解析到的 URL 列表: {:?}", task_id, urls);

                            if let Some(url) = urls
                                .into_iter()
                                .find(|u| u.starts_with("http://tlu.dl.delivery.mp.microsoft.com/"))
                            {
                                // 成功：把阶段设为 url_resolved（不在这里 finish task）
                                update_progress(task_id, 0, None, Some("url_resolved"));
                                return Ok(CoreResult::Success(url));
                            } else {
                                // 没有匹配的 URL
                                if attempt == 3 {
                                    finish_task(task_id, "error", Some("no matching url".into()));
                                    return Err(CoreError::BadUpdateIdentity);
                                } else {
                                    debug!("task={} 没有匹配 url，准备重试（attempt={})", task_id, attempt);
                                    // 继续重试（不要 finish_task）
                                }
                            }
                        }
                        Err(e) => {
                            debug!("task={} 第 {} 次请求返回错误状态: {}", task_id, attempt, e);
                            // 如果用户在这里取消了，尽早检测并返回 cancelled
                            if is_cancelled(task_id) {
                                debug!("task={} 已取消（HTTP error branch）", task_id);
                                finish_task(task_id, "cancelled", Some("user cancelled".into()));
                                return Ok(CoreResult::Cancelled);
                            }
                            if attempt == 3 {
                                // 最后一次失败：标 error 并返回
                                finish_task(task_id, "error", Some("http status error".into()));
                                return Err(CoreError::Request(e));
                            }
                            // 中间失败：记录并进入 backoff，继续尝试（不要 finish）
                        }
                    }
                }
                Err(err) => {
                    debug!("task={} 第 {} 次请求失败: {}", task_id, attempt, err);
                    // 再次检查是否被取消
                    if is_cancelled(task_id) {
                        debug!("task={} 已取消（request error branch）", task_id);
                        finish_task(task_id, "cancelled", Some("user cancelled".into()));
                        return Ok(CoreResult::Cancelled);
                    }
                    if attempt == 3 {
                        // 最后一次失败：标 error 并返回
                        finish_task(task_id, "error", Some(format!("request failed: {:?}", err)));
                        return Err(CoreError::Request(err));
                    }
                    // 中间失败：等待 backoff 然后重试（不要 finish）
                }
            }

            // backoff
            let backoff = 500 * attempt * attempt;
            // 在等待 backoff 期间也支持取消
            let task_id_wait = task_id.to_string();
            tokio::select! {
            _ = Self::wait_cancelled(task_id_wait.clone()) => {
                debug!("task={} 在 backoff 期间检测到取消", task_id);
                finish_task(task_id, "cancelled", Some("user cancelled".into()));
                return Ok(CoreResult::Cancelled);
            }
            _ = sleep(Duration::from_millis(backoff as u64)) => {}
        }
        }

        // 理论上不应到达这里
        finish_task(task_id, "error", Some("bad update identity".into()));
        Err(CoreError::BadUpdateIdentity)
    }
}
