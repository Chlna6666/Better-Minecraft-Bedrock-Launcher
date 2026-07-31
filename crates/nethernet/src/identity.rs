use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use p384::ecdsa::signature::Signer;
use p384::ecdsa::{Signature, SigningKey};
use p384::elliptic_curve::rand_core::OsRng;
use p384::pkcs8::EncodePublicKey;
use serde_json::{Value, json};

use crate::error::{NethernetError, Result};

const IDENTITY_LIFETIME_SECONDS: u64 = 60;

pub(crate) struct ServerIdentity {
    signing_key: SigningKey,
    public_key: String,
}

impl ServerIdentity {
    pub(crate) fn generate() -> Result<Self> {
        let signing_key = SigningKey::random(&mut OsRng);
        let public_key = signing_key
            .verifying_key()
            .to_public_key_der()
            .map_err(|error| {
                NethernetError::protocol(format!("编码 NetherNet 服务端公钥失败：{error}"))
            })?;
        Ok(Self {
            signing_key,
            public_key: STANDARD.encode(public_key.as_bytes()),
        })
    }

    pub(crate) fn attach_to_answer(&self, sdp: &str) -> Result<String> {
        let fingerprints = parse_fingerprints(sdp)?;
        let fingerprint_payload = json!({ "fingerprint": fingerprints }).to_string();
        let fingerprint_assertion =
            self.sign_detached(json!({ "alg": "ES384" }), fingerprint_payload.as_bytes())?;

        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|error| {
                NethernetError::protocol(format!("生成 NetherNet 身份时间戳失败：{error}"))
            })?
            .as_secs();
        let token = self.sign_compact(
            json!({ "alg": "ES384", "x5u": self.public_key }),
            json!({
                "cpk": self.public_key,
                "exp": now + IDENTITY_LIFETIME_SECONDS,
                "iat": now,
            })
            .to_string()
            .as_bytes(),
        )?;
        // go-nethernet 的 identityAssertion 实现了自定义 MarshalJSON：
        // assertion 在外层 JSON 中是一个包含 JSON 文本的字符串，而不是对象。
        // Minecraft 也按这个既有 wire format 解码。
        let assertion = json!({
            "fingerprints": fingerprint_assertion,
            "token": token,
        })
        .to_string();
        let identity = json!({
            "assertion": assertion,
            "idp": {
                "domain": "self",
                "protocol": "default",
            },
        });
        let attribute = format!("a=identity:{}\r\n", STANDARD.encode(identity.to_string()));
        insert_session_attribute(sdp, &attribute)
    }

    fn sign_detached(&self, header: Value, payload: &[u8]) -> Result<String> {
        let compact = self.sign_compact(header, payload)?;
        let mut parts = compact.split('.');
        let header = parts
            .next()
            .ok_or_else(|| NethernetError::protocol("NetherNet JWS 头缺失"))?;
        let _payload = parts
            .next()
            .ok_or_else(|| NethernetError::protocol("NetherNet JWS 载荷缺失"))?;
        let signature = parts
            .next()
            .ok_or_else(|| NethernetError::protocol("NetherNet JWS 签名缺失"))?;
        if parts.next().is_some() {
            return Err(NethernetError::protocol("NetherNet JWS 格式无效"));
        }
        Ok(format!("{header}..{signature}"))
    }

    fn sign_compact(&self, header: Value, payload: &[u8]) -> Result<String> {
        let header = serde_json::to_vec(&header).map_err(|error| {
            NethernetError::protocol(format!("编码 NetherNet JWS 头失败：{error}"))
        })?;
        let encoded_header = URL_SAFE_NO_PAD.encode(header);
        let encoded_payload = URL_SAFE_NO_PAD.encode(payload);
        let signing_input = format!("{encoded_header}.{encoded_payload}");
        let signature: Signature = self.signing_key.sign(signing_input.as_bytes());
        Ok(format!(
            "{signing_input}.{}",
            URL_SAFE_NO_PAD.encode(signature.to_bytes())
        ))
    }
}

fn parse_fingerprints(sdp: &str) -> Result<Vec<Value>> {
    let fingerprints: Vec<_> = sdp
        .lines()
        .filter_map(|line| line.trim().strip_prefix("a=fingerprint:"))
        .filter_map(|fingerprint| fingerprint.split_once(' '))
        .map(|(algorithm, digest)| {
            json!({
                "algorithm": algorithm,
                "digest": digest,
            })
        })
        .collect();
    if fingerprints.is_empty() {
        return Err(NethernetError::protocol(
            "NetherNet Answer 缺少 DTLS fingerprint",
        ));
    }
    Ok(fingerprints)
}

fn insert_session_attribute(sdp: &str, attribute: &str) -> Result<String> {
    if sdp.contains("\r\na=identity:") || sdp.starts_with("a=identity:") {
        return Ok(sdp.to_string());
    }
    let media_position = sdp
        .find("m=application")
        .ok_or_else(|| NethernetError::protocol("NetherNet Answer 缺少 application 媒体段"))?;
    let mut answer = String::with_capacity(sdp.len() + attribute.len());
    answer.push_str(&sdp[..media_position]);
    answer.push_str(attribute);
    answer.push_str(&sdp[media_position..]);
    Ok(answer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use p384::ecdsa::signature::Verifier;

    const SDP: &str = "v=0\r\n\
o=- 1 2 IN IP4 127.0.0.1\r\n\
s=-\r\n\
t=0 0\r\n\
a=group:BUNDLE 0\r\n\
a=fingerprint:sha-256 00:11:22:33:44:55\r\n\
m=application 9 UDP/DTLS/SCTP webrtc-datachannel\r\n\
a=sctp-port:5000\r\n";

    #[test]
    fn answer_identity_uses_gravitycone_wire_format_and_valid_signatures() {
        let identity = ServerIdentity::generate().expect("identity should generate");
        let answer = identity
            .attach_to_answer(SDP)
            .expect("identity should attach");
        let media_position = answer.find("m=application").expect("media section");
        let identity_position = answer.find("a=identity:").expect("identity attribute");
        let fingerprint_position = answer
            .find("a=fingerprint:")
            .expect("fingerprint attribute");
        assert!(fingerprint_position < identity_position);
        assert!(identity_position < media_position);

        let encoded = answer[identity_position + "a=identity:".len()..media_position].trim();
        let decoded = STANDARD.decode(encoded).expect("identity base64");
        let data: Value = serde_json::from_slice(&decoded).expect("identity json");
        let assertion: Value = serde_json::from_str(
            data["assertion"]
                .as_str()
                .expect("assertion should contain JSON text"),
        )
        .expect("assertion JSON");

        verify_compact(
            &identity,
            assertion["token"].as_str().expect("identity token"),
            None,
        );
        let payload = br#"{"fingerprint":[{"algorithm":"sha-256","digest":"00:11:22:33:44:55"}]}"#;
        verify_compact(
            &identity,
            assertion["fingerprints"]
                .as_str()
                .expect("fingerprint assertion"),
            Some(payload),
        );
    }

    #[test]
    fn answer_without_fingerprint_is_rejected() {
        let identity = ServerIdentity::generate().expect("identity should generate");
        let error = identity
            .attach_to_answer("v=0\r\nm=application 9 UDP/DTLS/SCTP\r\n")
            .expect_err("fingerprint is required");
        assert!(error.to_string().contains("fingerprint"));
    }

    fn verify_compact(identity: &ServerIdentity, compact: &str, detached_payload: Option<&[u8]>) {
        let parts: Vec<_> = compact.split('.').collect();
        assert_eq!(parts.len(), 3);
        let encoded_payload = detached_payload.map_or_else(
            || parts[1].to_string(),
            |payload| URL_SAFE_NO_PAD.encode(payload),
        );
        let signing_input = format!("{}.{}", parts[0], encoded_payload);
        let signature_bytes = URL_SAFE_NO_PAD.decode(parts[2]).expect("signature base64");
        let signature = Signature::from_slice(&signature_bytes).expect("ES384 signature");
        identity
            .signing_key
            .verifying_key()
            .verify(signing_input.as_bytes(), &signature)
            .expect("signature should verify");
    }
}
