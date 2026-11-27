//! OpenAI-compatible chat calls for the configured translation model.
//! Endpoint resolution follows the same rules as DBX: a bare origin gets `/v1`,
//! and `/chat/completions` is appended unless it is already there.

use std::time::Duration;

use base64::Engine;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiConfig {
    pub provider: String,
    pub endpoint: String,
    pub api_key: String,
    pub model: String,
}

const TRANSLATE_SYSTEM: &str = "Translate each numbered line into Simplified Chinese. Keep the same numbers and the same count of lines. Output only those lines. Wrap at most two key terms per line in 【】.";

const READ_SYSTEM: &str = "You can see the screenshot. Read every text line in reading order. Reply with the original text only, one visual line per line. Do not translate. Do not add commentary.";

const SEE_SYSTEM: &str = "You can see the screenshot. Read every text line and translate it into natural Simplified Chinese. Reply with one line per region and nothing else. Format: left,top,right,bottom|简体中文. The four numbers are integers from 0 to 1000, a tight box around the full line relative to the image width and height. Keep product names and code that should stay in English. Wrap at most two key terms per line in 【】, never a whole line. Do not number the lines.";

pub fn chat_url(endpoint: &str) -> Result<String, String> {
    let ep = endpoint.trim().trim_end_matches('/');
    if ep.is_empty() {
        return Err("请填写接口地址".into());
    }
    if let Some(base) = ep.strip_suffix("/chat/completions").or_else(|| ep.strip_suffix("/responses")) {
        return Ok(format!("{}/chat/completions", base.trim_end_matches('/')));
    }
    if ep.ends_with("/messages") {
        return Err("暂只支持 OpenAI 兼容接口".into());
    }
    let base = if ep.ends_with("/v1") {
        ep.to_string()
    } else {
        let origin_only = match ep.split_once("://") {
            Some((_, rest)) => !rest.contains('/'),
            None => !ep.contains('/'),
        };
        if origin_only { format!("{ep}/v1") } else { ep.to_string() }
    };
    Ok(format!("{base}/chat/completions"))
}

pub fn models_url(endpoint: &str) -> Result<String, String> {
    let chat = chat_url(endpoint)?;
    let Some(base) = chat.strip_suffix("/chat/completions") else {
        return Err("请填写接口地址".into());
    };
    Ok(format!("{base}/models"))
}

pub fn list_models(config: &AiConfig) -> Result<Vec<String>, String> {
    if config.endpoint.trim().is_empty() {
        return Err("请填写接口地址".into());
    }
    if config.api_key.trim().is_empty() && !allows_empty_key(config) {
        return Err("请先填写 API Key".into());
    }
    let url = models_url(&config.endpoint)?;
    let key = config.api_key.trim();
    let (status, text) = http_send("GET", &url, key, None)?;
    if status >= 400 {
        return Err(format!("接口返回 {status}：{}", clip(&text)));
    }
    let value: serde_json::Value = serde_json::from_str(&text).map_err(|error| format!("接口返回无法解析：{error}"))?;
    let mut names = Vec::new();
    if let Some(rows) = value.get("data").and_then(|data| data.as_array()) {
        for row in rows {
            if let Some(id) = row.get("id").and_then(|id| id.as_str()) {
                let id = id.trim();
                if !id.is_empty() {
                    names.push(id.to_string());
                }
            }
        }
    }
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Err("接口没有返回模型".into());
    }
    Ok(names)
}

pub fn test_connection(config: &AiConfig) -> Result<String, String> {
    let reply = complete(config, "Reply with the single word ok.", "ping", 16)?;
    if reply.is_empty() {
        return Err("接口没有返回内容".into());
    }
    Ok("连接成功".into())
}

pub fn see(config: &AiConfig, png: &[u8], mode: &str) -> Result<String, String> {
    ensure(config)?;
    let png = shrink_png(png);
    let encoded = base64::engine::general_purpose::STANDARD.encode(png);
    let reading = mode == "ocr";
    let mut body = serde_json::json!({
        "model": config.model.trim(),
        "temperature": 0,
        "max_tokens": 1500,
        "messages": [
            { "role": "system", "content": if reading { READ_SYSTEM } else { SEE_SYSTEM } },
            { "role": "user", "content": [
                { "type": "text", "text": if reading { "Read this screenshot." } else { "Read this screenshot and reply in the required format." } },
                { "type": "image_url", "image_url": { "url": format!("data:image/png;base64,{encoded}") } }
            ] }
        ]
    });
    disable_thinking(&mut body, config);
    tune_model(&mut body, config);
    match post(config, body, true) {
        Err(message) if vision_rejected(&message) => {
            Err("这个模型不能看图。请换成支持视觉的模型，例如 qwen-vl-max 或 gpt-4o。".into())
        }
        other => other,
    }
}

pub fn translate(config: &AiConfig, text: &str) -> Result<String, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(String::new());
    }
    let started = std::time::Instant::now();
    eprintln!("ai: translate start model={} chars={}", config.model.trim(), text.chars().count());
    let mut parts = Vec::new();
    for chunk in chunks(text, 8000) {
        let budget = token_budget(&chunk);
        match complete(config, TRANSLATE_SYSTEM, &chunk, budget) {
            Ok(part) => parts.push(part),
            Err(error) => {
                eprintln!("ai: translate fail {}ms {error}", started.elapsed().as_millis());
                return Err(error);
            }
        }
    }
    let translated = parts.join("\n");
    eprintln!("ai: translate ok {}ms chars={}", started.elapsed().as_millis(), translated.chars().count());
    Ok(translated)
}

fn complete(config: &AiConfig, system: &str, user: &str, max_tokens: u32) -> Result<String, String> {
    ensure(config)?;
    let mut body = serde_json::json!({
        "model": config.model.trim(),
        "temperature": 0,
        "max_tokens": max_tokens,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ]
    });
    disable_thinking(&mut body, config);
    tune_model(&mut body, config);
    post(config, body, false)
}

fn tune_model(body: &mut serde_json::Value, config: &AiConfig) {
    if config.model.to_ascii_lowercase().contains("grok") {
        body["reasoning_effort"] = serde_json::json!("none");
    }
}

fn ensure(config: &AiConfig) -> Result<(), String> {
    if config.model.trim().is_empty() {
        return Err("请填写模型".into());
    }
    if config.api_key.trim().is_empty() && !allows_empty_key(config) {
        return Err("请先在设置里填写 API Key 并保存".into());
    }
    Ok(())
}

fn post(config: &AiConfig, body: serde_json::Value, _slow: bool) -> Result<String, String> {
    let url = chat_url(&config.endpoint)?;
    let key = config.api_key.trim();
    let payload = serde_json::to_vec(&body).map_err(|error| error.to_string())?;
    let (status, text) = http_send("POST", &url, key, Some(&payload))?;
    parse_http_json(status, &text)
}

fn parse_http_json(status: u32, text: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|error| format!("接口返回无法解析：{error}"))?;
    if status >= 400 {
        let detail = value.get("error").and_then(|error| error.get("message")).and_then(|message| message.as_str()).unwrap_or(text);
        return Err(format!("接口返回 {status}：{}", clip(detail)));
    }
    if let Some(message) = value.get("error").and_then(|error| error.get("message")).and_then(|message| message.as_str()) {
        let message = message.trim();
        if !message.is_empty() {
            return Err(clip(message));
        }
    }
    message_text(&value).ok_or_else(|| empty_reply(&value))
}

fn vision_rejected(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("image") || lower.contains("vision") || lower.contains("multimodal") || lower.contains("image_url")
}

fn shrink_png(bytes: &[u8]) -> Vec<u8> {
    let Ok(picture) = image::load_from_memory(bytes) else {
        return bytes.to_vec();
    };
    const MAX: u32 = 1024;
    let (width, height) = (picture.width(), picture.height());
    if width <= MAX && height <= MAX {
        return bytes.to_vec();
    }
    let scale = MAX as f32 / width.max(height) as f32;
    let width = ((width as f32) * scale).floor().max(1.0) as u32;
    let height = ((height as f32) * scale).floor().max(1.0) as u32;
    let resized = picture.resize(width, height, image::imageops::FilterType::Triangle);
    let mut encoded = std::io::Cursor::new(Vec::new());
    if resized.write_to(&mut encoded, image::ImageFormat::Png).is_err() {
        return bytes.to_vec();
    }
    encoded.into_inner()
}

fn vision_agent() -> ureq::Agent {
    http_agent(Duration::from_secs(30))
}

/// Thinking models spend the output budget before writing `content`. Translation does not need that.
fn disable_thinking(body: &mut serde_json::Value, config: &AiConfig) {
    let model = config.model.to_ascii_lowercase();
    // Grok and GPT reject or stall on DeepSeek's thinking field. Only send it to models that use it.
    if model.contains("grok") || model.contains("gpt") || model.contains("claude") {
        return;
    }
    match config.provider.as_str() {
        "openai" => {}
        "minimax" if config.model.trim().eq_ignore_ascii_case("MiniMax-M3.1-Flash") => {}
        "ollama" => body["reasoning_effort"] = serde_json::json!("none"),
        "qwen" => body["enable_thinking"] = serde_json::json!(false),
        "deepseek" | "minimax" | "kimi" | "zhipu" => body["thinking"] = serde_json::json!({ "type": "disabled" }),
        _ => {}
    }
}

fn empty_reply(value: &serde_json::Value) -> String {
    let choice = &value["choices"][0];
    let thought = choice["message"]["reasoning_content"].as_str().is_some_and(|text| !text.trim().is_empty());
    if thought || choice["finish_reason"].as_str() == Some("length") {
        return "模型只返回了思考过程，没有译文。请换成不思考的模型，例如 deepseek-chat。".into();
    }
    "模型没有返回译文".into()
}

fn allows_empty_key(config: &AiConfig) -> bool {
    if config.provider == "ollama" {
        return true;
    }
    let endpoint = config.endpoint.to_ascii_lowercase();
    endpoint.contains("localhost") || endpoint.contains("127.0.0.1")
}

fn message_text(value: &serde_json::Value) -> Option<String> {
    let choice = value.get("choices")?.get(0)?;
    let content = choice.get("message").and_then(|message| message.get("content")).or_else(|| choice.get("text"))?;
    let text = match content {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Array(parts) => parts.iter().filter_map(|part| part.get("text")).filter_map(|text| text.as_str()).collect(),
        _ => return None,
    };
    let text = text.trim();
    if text.is_empty() { None } else { Some(text.to_string()) }
}

fn chunks(text: &str, limit: usize) -> Vec<String> {
    let mut parts = Vec::new();
    let mut rest = text.trim();
    while rest.chars().count() > limit {
        let mut cut = 0;
        let mut chars = 0;
        let mut last_break = 0;
        for (index, ch) in rest.char_indices() {
            chars += 1;
            if ch == '\n' {
                last_break = index + ch.len_utf8();
            }
            if chars >= limit {
                cut = if last_break > limit / 2 { last_break } else { index };
                break;
            }
        }
        if cut == 0 {
            cut = rest.len();
        }
        parts.push(rest[..cut].trim().to_string());
        rest = rest[cut..].trim_start();
    }
    if !rest.is_empty() {
        parts.push(rest.to_string());
    }
    parts
}

fn token_budget(text: &str) -> u32 {
    let count = text.chars().count() as u32;
    count.saturating_add(48).clamp(64, 700)
}

fn agent() -> ureq::Agent {
    http_agent(Duration::from_secs(30))
}

/// The system proxy is on, and the API host is a fake-ip (`198.18.0.0/15`).
/// A direct connect waits until Windows error 10060. Browser traffic uses the proxy.
fn http_agent(timeout: Duration) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new().timeout(timeout);
    if let Some(proxy) = system_proxy() {
        builder = builder.proxy(proxy);
    }
    builder.build()
}

fn system_proxy() -> Option<ureq::Proxy> {
    let url = env_proxy().or_else(windows_proxy)?;
    eprintln!("ai: using proxy {url}");
    ureq::Proxy::new(&url).ok()
}

fn env_proxy() -> Option<String> {
    let value = std::env::var("HTTPS_PROXY")
        .or_else(|_| std::env::var("https_proxy"))
        .or_else(|_| std::env::var("ALL_PROXY"))
        .or_else(|_| std::env::var("all_proxy"))
        .ok()?;
    proxy_url(&value)
}

#[cfg(windows)]
fn windows_proxy() -> Option<String> {
    use winreg::enums::HKEY_CURRENT_USER;
    use winreg::RegKey;
    let settings = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings")
        .ok()?;
    let enabled: u32 = settings.get_value("ProxyEnable").unwrap_or(0);
    if enabled == 0 {
        eprintln!("ai: system proxy is off");
        return None;
    }
    let server: String = settings.get_value("ProxyServer").ok()?;
    proxy_url(&server)
}

#[cfg(not(windows))]
fn windows_proxy() -> Option<String> {
    None
}

fn proxy_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let server = if raw.contains('=') {
        raw.split(';').find_map(|part| {
            let (key, value) = part.split_once('=')?;
            (key.eq_ignore_ascii_case("https") || key.eq_ignore_ascii_case("http")).then_some(value.trim())
        })?
    } else {
        raw
    };
    if server.contains("://") {
        Some(server.to_string())
    } else {
        Some(format!("http://{server}"))
    }
}

fn http_send(method: &str, url: &str, key: &str, body: Option<&[u8]>) -> Result<(u32, String), String> {
    #[cfg(windows)]
    {
        winhttp_send(method, url, key, body)
    }
    #[cfg(not(windows))]
    {
        let client = agent();
        let mut request = match method {
            "GET" => client.get(url),
            _ => client.post(url).set("Content-Type", "application/json"),
        };
        if !key.is_empty() {
            request = request.set("Authorization", &format!("Bearer {key}"));
        }
        let response = if let Some(body) = body {
            request.send_bytes(body)
        } else if method == "GET" {
            request.call()
        } else {
            request.send_bytes(&[])
        };
        match response {
            Ok(response) => {
                let status = response.status();
                let text = response.into_string().map_err(|error| error.to_string())?;
                Ok((status, text))
            }
            Err(ureq::Error::Status(status, response)) => {
                let text = response.into_string().unwrap_or_default();
                Ok((status, text))
            }
            Err(error) => Err(format!("连不上接口：{error}")),
        }
    }
}

#[cfg(windows)]
fn winhttp_send(method: &str, url: &str, key: &str, body: Option<&[u8]>) -> Result<(u32, String), String> {
    use windows::core::{PCWSTR, w};
    use windows::Win32::Networking::WinHttp::{
        WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest, WinHttpQueryDataAvailable,
        WinHttpQueryHeaders, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest, WinHttpSetTimeouts,
        WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ACCESS_TYPE_NAMED_PROXY, WINHTTP_ADDREQ_FLAG_ADD, WINHTTP_FLAG_SECURE,
        WINHTTP_QUERY_FLAG_NUMBER, WINHTTP_QUERY_STATUS_CODE,
    };

    struct Handle(*mut core::ffi::c_void);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe { let _ = WinHttpCloseHandle(self.0); }
            }
        }
    }

    let (https, host, port, path) = split_http_url(url)?;
    let host_w = utf16(&host);
    let path_w = utf16(&path);
    let verb_w = utf16(method);
    let proxy = windows_proxy().unwrap_or_default();
    let proxy_host = proxy.trim_start_matches("http://").trim_start_matches("https://").trim_end_matches('/').to_string();
    let proxy_w = utf16(&proxy_host);
    eprintln!("ai: winhttp {method} {url} proxy={}", if proxy_host.is_empty() { "auto" } else { &proxy_host });

    unsafe {
        let session = Handle(if proxy_host.is_empty() {
            WinHttpOpen(w!("Shotlet"), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0)
        } else {
            WinHttpOpen(w!("Shotlet"), WINHTTP_ACCESS_TYPE_NAMED_PROXY, PCWSTR(proxy_w.as_ptr()), PCWSTR::null(), 0)
        });
        if session.0.is_null() {
            return Err("无法打开网络连接".into());
        }
        WinHttpSetTimeouts(session.0, 8000, 10000, 20000, 60000).map_err(|error| error.to_string())?;
        let connect = Handle(WinHttpConnect(session.0, PCWSTR(host_w.as_ptr()), port, 0));
        if connect.0.is_null() {
            return Err(format!("连不上 {host}"));
        }
        let flags = if https { WINHTTP_FLAG_SECURE } else { windows::Win32::Networking::WinHttp::WINHTTP_OPEN_REQUEST_FLAGS(0) };
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            PCWSTR(verb_w.as_ptr()),
            PCWSTR(path_w.as_ptr()),
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            flags,
        ));
        if request.0.is_null() {
            return Err("无法创建请求".into());
        }
        let mut headers = format!("Content-Type: application/json\r\n");
        if !key.is_empty() {
            headers.push_str(&format!("Authorization: Bearer {key}\r\n"));
        }
        let headers_w: Vec<u16> = headers.encode_utf16().collect();
        WinHttpAddRequestHeaders(request.0, &headers_w, WINHTTP_ADDREQ_FLAG_ADD).map_err(|error| error.to_string())?;
        let length = body.map(|bytes| bytes.len() as u32).unwrap_or(0);
        WinHttpSendRequest(
            request.0,
            None,
            body.map(|bytes| bytes.as_ptr() as *const _),
            length,
            length,
            0,
        )
        .map_err(|error| error.to_string())?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut()).map_err(|error| error.to_string())?;
        let mut status = 0u32;
        let mut size = 4u32;
        let mut index = 0u32;
        WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some(&mut status as *mut u32 as *mut _),
            &mut size,
            &mut index,
        )
        .map_err(|error| error.to_string())?;
        let mut text = Vec::new();
        loop {
            let mut available = 0u32;
            WinHttpQueryDataAvailable(request.0, &mut available).map_err(|error| error.to_string())?;
            if available == 0 {
                break;
            }
            let mut chunk = vec![0u8; available as usize];
            let mut read = 0u32;
            WinHttpReadData(request.0, chunk.as_mut_ptr() as *mut _, available, &mut read).map_err(|error| error.to_string())?;
            chunk.truncate(read as usize);
            text.extend_from_slice(&chunk);
        }
        let text = String::from_utf8_lossy(&text).into_owned();
        eprintln!("ai: winhttp status={status} bytes={}", text.len());
        Ok((status, text))
    }
}

fn split_http_url(url: &str) -> Result<(bool, String, u16, String), String> {
    let (scheme, rest) = url.split_once("://").ok_or_else(|| format!("无效的接口地址：{url}"))?;
    let https = scheme == "https";
    let (hostport, path) = rest.split_once('/').map(|(host, path)| (host, format!("/{path}"))).unwrap_or((rest, "/".into()));
    let (host, port) = match hostport.rsplit_once(':') {
        Some((host, port)) if !host.is_empty() && port.chars().all(|ch| ch.is_ascii_digit()) => {
            (host.to_string(), port.parse::<u16>().map_err(|error| error.to_string())?)
        }
        _ => (hostport.to_string(), if https { 443 } else { 80 }),
    };
    Ok((https, host, port, path))
}

fn utf16(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn http_error(error: ureq::Error) -> String {
    match error {
        ureq::Error::Status(code, response) => {
            let body = response.into_string().unwrap_or_default();
            let detail = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|value| {
                    value.get("error")
                        .and_then(|error| error.get("message"))
                        .and_then(|message| message.as_str())
                        .map(str::trim)
                        .filter(|message| !message.is_empty())
                        .map(clip)
                })
                .unwrap_or_else(|| {
                    let trimmed = body.trim();
                    if trimmed.is_empty() { format!("HTTP {code}") } else { clip(trimmed) }
                });
            format!("接口返回 {code}：{detail}")
        }
        ureq::Error::Transport(error) => format!("连不上接口：{error}"),
    }
}

fn clip(text: &str) -> String {
    text.chars().take(180).collect()
}

#[cfg(test)]
mod tests {
    use super::{chat_url, models_url};

    #[test]
    fn chat_url_matches_openai_compatible_bases() {
        assert_eq!(chat_url("https://api.deepseek.com/v1").unwrap(), "https://api.deepseek.com/v1/chat/completions");
        assert_eq!(chat_url("https://api.deepseek.com").unwrap(), "https://api.deepseek.com/v1/chat/completions");
        assert_eq!(
            chat_url("https://api.openai.com/v1/chat/completions").unwrap(),
            "https://api.openai.com/v1/chat/completions"
        );
        assert_eq!(
            chat_url("https://dashscope.aliyuncs.com/compatible-mode/v1").unwrap(),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
        );
        assert_eq!(
            chat_url("https://open.bigmodel.cn/api/paas/v4").unwrap(),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert!(chat_url("  ").is_err());
        assert_eq!(models_url("https://cpa.htmlto.link/v1").unwrap(), "https://cpa.htmlto.link/v1/models");
    }

    #[test]
    fn empty_reasoning_is_not_a_translation() {
        let value = serde_json::json!({
            "choices": [{
                "finish_reason": "length",
                "message": { "content": null, "reasoning_content": "先分析这句话" }
            }]
        });
        assert!(super::message_text(&value).is_none());
        assert!(super::empty_reply(&value).contains("思考"));
    }
}
