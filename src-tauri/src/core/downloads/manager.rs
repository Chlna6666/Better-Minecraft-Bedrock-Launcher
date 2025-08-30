use reqwest::Client;
use std::path::Path;
use tokio::time::{sleep, Duration};
use crate::core::downloads::single;
use crate::core::downloads::multi;
use crate::core::result::{CoreError, CoreResult};

pub struct DownloaderManager {
    client: Client,
}

impl DownloaderManager {
    pub fn with_client(client: Client) -> Self {
        Self { client }
    }

    pub async fn download(
        &self,
        url: String,
        dest: impl AsRef<Path>,
        threads: usize,
    ) -> Result<CoreResult, CoreError> {
        let mut retry = 0;
        loop {
            let res = if threads > 1 {
                multi::download_multi(self.client.clone(), &url, &dest,  threads).await
            } else {
                single::download_file(self.client.clone(), &url, &dest).await
            };

            match &res {
                Ok(CoreResult::Success(_)) | Ok(CoreResult::Cancelled) => return res,
                _ if retry < 3 => {
                    retry += 1;
                    sleep(Duration::from_secs(1)).await;
                }
                _ => return res,
            }
        }
    }
}