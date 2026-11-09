//! Redaction at the final logging sink: apply this to both managed logs and consoles.
//! Avoid transforming business data; this protects *diagnostic output* only.
use once_cell::sync::Lazy;
use regex::Regex;
use std::borrow::Cow;

static URL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r#"(?i)\b[a-z][a-z0-9+.-]{1,20}://[^\s"'<>]+"#).expect("valid URL regex")
});
static SECRET: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)\b(?:authorization|access[_-]?token|refresh[_-]?token|id[_-]?token|auth[_-]?token|password|passwd|secret|client[_-]?secret|api[_-]?key|apikey|cookie|session[_-]?key|xsts|xuid|uhs|user[_-]?id|username|gamertag|display_name|world_root|target_exe_path)\b["']?\s*[:=]\s*(?:Some\([^)\r\n]*\)|"[^"\r\n]*"|'[^'\r\n]*'|[^\s,;}\]]+)"#,
    ).expect("valid secret-field regex")
});
static BEARER: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b(?:bearer|basic)\s+[A-Za-z0-9_~+/=.-]{8,}").expect("valid bearer regex")
});
static EMAIL: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)\b[A-Z0-9._%+-]+@[A-Z0-9.-]+\.[A-Z]{2,}\b").expect("valid email regex")
});
static QUOTED_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)"(?:[a-z]:\\+|\\\\+|/(?:home|users|root|tmp|mnt|media|private)/)[^"\r\n]*"|'(?:[a-z]:\\+|\\\\+|/(?:home|users|root|tmp|mnt|media|private)/)[^'\r\n]*'"#,
    ).expect("valid quoted-path regex")
});
static PATH_FIELD: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)\b(?:path|file_path|file_name|filename|directory|dir|source_path|target_path|output_path|input_path|src|dst|home_dir|user_dir)\b["']?\s*[:=]\s*(?:"[^"\r\n]*"|'[^'\r\n]*'|[^\s,;]+)"#,
    ).expect("valid path-field regex")
});
static ABSOLUTE_PATH: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r#"(?i)(?:[a-z]:\\+|\\\\+|/(?:home|users|root|tmp|mnt|media|private)/)[^\s"'<>\]\)\},;]+"#,
    ).expect("valid absolute-path regex")
});

/// Protect log files, DevTools' latest.log tail, and the optional OS console.
/// The regular expressions execute on the asynchronous file-writer thread for
/// managed logs, never on UI, scene, download-progress or GPU-owner hot paths.
pub(crate) fn redact_log_text(input: &str) -> Cow<'_, str> {
    if input.is_empty() {
        return Cow::Borrowed(input);
    }
    // Each Cow must outlive the next replacement: rebinding a Cow which may
    // borrow from its previous value fails lifetime checking.
    // Bearer credentials must be removed *before* redacting Authorization
    // itself, otherwise "Authorization: Bearer <token>" can leave the token.
    let bearer = BEARER.replace_all(input, "[认证信息]");
    let secret = SECRET.replace_all(bearer.as_ref(), "[敏感字段]");
    let urls = URL.replace_all(secret.as_ref(), "[URL]");
    let emails = EMAIL.replace_all(urls.as_ref(), "[邮箱]");
    let path_fields = PATH_FIELD.replace_all(emails.as_ref(), "[本地路径]");
    let quoted_paths = QUOTED_PATH.replace_all(path_fields.as_ref(), "[本地路径]");
    let paths = ABSOLUTE_PATH.replace_all(quoted_paths.as_ref(), "[本地路径]");
    Cow::Owned(paths.into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_credentials_urls_paths_and_email() {
        let example = r#"source_url=https://example.com/private?token=abcd access_token=abcd1234 Authorization:Bearer-secret username=alice path="C:\Users\Alice\My Worlds\private.mcworld" /home/alice/.config/secret test@example.org"#;
        let output = redact_log_text(example);
        for secret in [
            "example.com", "abcd1234", "alice", "My Worlds", "/home/alice",
            "test@example.org",
        ] {
            assert!(!output.to_lowercase().contains(&secret.to_lowercase()), "{output}");
        }
        assert!(output.contains("[URL]"));
        assert!(output.contains("[本地路径]"));
    }

    #[test]
    fn redacts_json_credentials_and_spaced_bearer_tokens() {
        let text = r#"Authorization: Bearer eyJ0eXAiOiJKV1QiLCJhbGciOiJIUzI1NiJ9 {"password": "private phrase", "access_token":"secret-value"} filename="private user world.mcworld""#;
        let output = redact_log_text(text);
        for value in ["eyJ0eXAi", "private phrase", "secret-value", "private user world"] {
            assert!(!output.contains(value), "{output}");
        }
    }

    #[test]
    fn keeps_task_identifiers_and_progress_metrics() {
        let line = "INFO bmcbl::operation task_id=task-13-17000 stage=map_export chunks=128 elapsed_ms=240";
        assert_eq!(redact_log_text(line).as_ref(), line);
    }
}
