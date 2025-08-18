//! Windows Update协议客户端实现
//!
//! 本实现参考了GPLv3许可的C#项目 mc-w10-version-launcher (https://github.com/MCMrARM/mc-w10-version-launcher)
//!
//! 原始C#项目采用GPLv3许可，本项目使用 Rust实现，采用GPLv3许可
use xml::reader::{EventReader, XmlEvent};
use uuid::Uuid;
use chrono::{DateTime, Utc};
use crate::core::result::CoreError;

#[derive(Clone)]
pub struct WuProtocol {
    pub msa_token: Option<String>,
}

impl WuProtocol {
    pub fn new() -> Self {
        Self { msa_token: None }
    }
    
    pub fn build_download_request(&self, update_id: &str, revision: &str) -> String {
        let now: DateTime<Utc> = Utc::now();
        let created = now.to_rfc3339();
        let expires = (now + chrono::Duration::minutes(5)).to_rfc3339();
        format!(
            r#"<s:Envelope xmlns:a="http://www.w3.org/2005/08/addressing"
    xmlns:s="http://www.w3.org/2003/05/soap-envelope"
    xmlns:wu="http://www.microsoft.com/SoftwareDistribution/Server/ClientWebService">
  <s:Header>
    <a:Action s:mustUnderstand="1">http://www.microsoft.com/SoftwareDistribution/Server/ClientWebService/GetExtendedUpdateInfo2</a:Action>
    <a:MessageID>urn:uuid:{}</a:MessageID>
    <a:To s:mustUnderstand="1">https://fe3.delivery.mp.microsoft.com/ClientWebService/client.asmx/secured</a:To>
    <o:Security s:mustUnderstand="1" xmlns:o="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd">
      <u:Timestamp xmlns:u="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd">
        <u:Created>{}</u:Created>
        <u:Expires>{}</u:Expires>
      </u:Timestamp>
      {}
    </o:Security>
  </s:Header>
  <s:Body>
    <wu:GetExtendedUpdateInfo2>
      <wu:updateIDs>
        <wu:UpdateIdentity>
          <wu:UpdateID>{}</wu:UpdateID>
          <wu:RevisionNumber>{}</wu:RevisionNumber>
        </wu:UpdateIdentity>
      </wu:updateIDs>
      <wu:infoTypes>
        <wu:XmlUpdateFragmentType>FileUrl</wu:XmlUpdateFragmentType>
      </wu:infoTypes>
      <wu:deviceAttributes>...</wu:deviceAttributes>
    </wu:GetExtendedUpdateInfo2>
  </s:Body>
</s:Envelope>"#,
            Uuid::new_v4(),
            created,
            expires,
            self.build_windows_update_tickets(),
            update_id,
            revision
        )
    }

    /// 生成票据部分，参考 C# 的实现
    pub fn build_windows_update_tickets(&self) -> String {
        let mut tickets = String::new();
        tickets.push_str(
            r#"<wuws:WindowsUpdateTicketsToken wsu:id="ClientMSA"
    xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd"
    xmlns:wuws="http://schemas.microsoft.com/msus/2014/10/WindowsUpdateAuthorization">"#,
        );
        if let Some(token) = &self.msa_token {
            tickets.push_str(&format!(
                r#"<TicketType Name="MSA" Version="1.0" Policy="MBI_SSL"><User>{}</User></TicketType>"#,
                token
            ));
        } else {
            tickets.push_str(r#"<TicketType Name="MSA" Version="1.0" Policy="MBI_SSL"/>"#);
        }
        tickets.push_str(r#"<TicketType Name="AAD" Version="1.0" Policy="MBI_SSL"/>"#);
        tickets.push_str(r#"</wuws:WindowsUpdateTicketsToken>"#);
        tickets
    }

    /// 利用 EventReader 解析响应 XML 中的 URL
    pub fn parse_download_response(&self, xml: &str) -> Result<Vec<String>, CoreError> {
        let parser = EventReader::from_str(xml);
        let mut urls = Vec::new();
        let mut in_file_location = false;
        let mut in_url = false;
        for event in parser {
            match event {
                Ok(XmlEvent::StartElement { name, .. }) => {
                    if name.local_name == "FileLocation" {
                        in_file_location = true;
                    } else if in_file_location && name.local_name == "Url" {
                        in_url = true;
                    }
                }
                Ok(XmlEvent::EndElement { name }) => {
                    if name.local_name == "FileLocation" {
                        in_file_location = false;
                    } else if name.local_name == "Url" {
                        in_url = false;
                    }
                }
                Ok(XmlEvent::Characters(text)) => {
                    if in_url {
                        urls.push(text.trim().to_string());
                    }
                }
                Err(e) => {
                    return Err(CoreError::Xml(e));
                }
                _ => {}
            }
        }
        Ok(urls)
    }
}
